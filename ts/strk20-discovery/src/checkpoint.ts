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
    // Fetch by the same height concurrently. Rust binds the proof's block hash
    // and global roots to this independently trusted header, including reorgs.
    const [header, proof] = await Promise.all([
      this.net.rpc(this.opts.rpcUrl, "starknet_getBlockWithTxHashes", [
        { block_number: block },
      ]),
      this.proof(block),
      this.checkChain(),
    ]) as [{ block_number: number; block_hash: string; new_root: string;
      status: string }, string, void];
    if (
      header.block_number !== block ||
      !["ACCEPTED_ON_L1", "ACCEPTED_ON_L2"].includes(header.status)
    )
      throw new Error("CHECKPOINT_UNAVAILABLE: accepted block header required");
    const cp = {
      chain_id: this.profile.chainId,
      pool: hex(this.profile.pool),
      block_number: block,
      block_hash: header.block_hash,
      state_root: header.new_root,
    };
    return { checkpoint: cp, proof };
  }
  private async checkChain(): Promise<void> {
    if (this.chainChecked) return;
    const id = await this.net.rpc(this.opts.rpcUrl, "starknet_chainId", []);
    const expected = `0x${[...this.profile.chainId].map((c) => c.charCodeAt(0).toString(16)).join("")}`;
    if (typeof id !== "string" || hex(id) !== hex(expected))
      throw new Error("CHAIN_MISMATCH: checkpoint RPC");
    this.chainChecked = true;
  }
  private async proof(block: number): Promise<string> {
    for (let attempt = 0; ; attempt++) {
      try {
        return JSON.stringify(await this.net.rpc(
          this.opts.proofRpcUrl,
          "starknet_getStorageProof",
          [{ block_number: block }, [], [hex(this.profile.pool)], []],
        ));
      } catch (e) {
        if (attempt === 2 || !/RPC_UNAVAILABLE: (42|24|-32603)/.test(String(e)))
          throw e;
      }
    }
  }
}
