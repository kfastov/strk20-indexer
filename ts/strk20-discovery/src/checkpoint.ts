import type { Checkpoint, ChainProfile, RuntimeOptions } from "./types.ts";
import { PublicTransport } from "./net.ts";
const hex = (value: string) => `0x${BigInt(value).toString(16)}`;

/** Independent RPC header selection and proof acquisition; Rust verifies the proof. */
export class CheckpointSource {
  private chainChecked = false;
  private readonly net: PublicTransport;
  private readonly opts: RuntimeOptions;
  private readonly profile: ChainProfile;
  constructor(
    net: PublicTransport,
    opts: RuntimeOptions,
    profile: ChainProfile,
  ) {
    this.net = net;
    this.opts = opts;
    this.profile = profile;
  }
  async acquire(
    block: number,
  ): Promise<{ checkpoint: Checkpoint; proof: string }> {
    if (!this.chainChecked) {
      const id = await this.net.rpc(this.opts.rpcUrl, "starknet_chainId", []);
      const expected = `0x${[...this.profile.chainId].map((c) => c.charCodeAt(0).toString(16)).join("")}`;
      if (typeof id !== "string" || hex(id) !== hex(expected))
        throw new Error("CHAIN_MISMATCH: checkpoint RPC");
      this.chainChecked = true;
    }
    const number = block;
    const header = (await this.net.rpc(
      this.opts.rpcUrl,
      "starknet_getBlockWithTxHashes",
      [{ block_number: number }],
    )) as {
      block_number: number;
      block_hash: string;
      new_root: string;
      status: string;
    };
    if (
      header.block_number !== number ||
      !["ACCEPTED_ON_L1", "ACCEPTED_ON_L2"].includes(header.status)
    )
      throw new Error("CHECKPOINT_UNAVAILABLE: accepted block header required");
    const cp = {
      chain_id: this.profile.chainId,
      pool: hex(this.profile.pool),
      block_number: number,
      block_hash: header.block_hash,
      state_root: header.new_root,
    };
    let proof: unknown;
    for (let attempt = 0; attempt < 3; attempt++) {
      try {
        proof = await this.net.rpc(
          this.opts.proofRpcUrl,
          "starknet_getStorageProof",
          [{ block_hash: header.block_hash }, [], [hex(this.profile.pool)], []],
        );
        break;
      } catch (e) {
        if (attempt === 2 || !/RPC_UNAVAILABLE: (42|24|-32603)/.test(String(e)))
          throw e;
      }
    }
    return { checkpoint: cp, proof: JSON.stringify(proof) };
  }
}
