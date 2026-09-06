import type { Checkpoint, ChainProfile, RuntimeOptions } from "./types.ts";
import { PublicTransport } from "./net.ts";
const hex = (value: string) => `0x${BigInt(value).toString(16)}`;
type Acquired = { checkpoint: Checkpoint; proof: string };

/** Independent RPC header selection and proof acquisition; Rust verifies the proof. */
export class CheckpointSource {
  private chainChecked = false;
  private pending: { block: number; work: Promise<Acquired>; abort: AbortController } | undefined;
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
  /** One bounded speculative read: overlap RPC with feed arrival and folding. */
  prepare(block: number): void {
    if (this.pending?.block === block) return;
    this.cancel();
    const abort = new AbortController();
    const pending = { block, abort, work: this.fetch(block, abort.signal) };
    this.pending = pending;
    // Preparation can outlive a BOUND_UNAVAILABLE result. Handle rejection
    // immediately and allow the next read to retry an unavailable checkpoint.
    void pending.work.catch(() => {
      pending.abort.abort();
      if (this.pending === pending) this.pending = undefined;
    });
  }
  cancel(): void {
    this.pending?.abort.abort();
    this.pending = undefined;
  }
  async acquire(block: number): Promise<Acquired> {
    this.prepare(block);
    const pending = this.pending!;
    try { return await pending.work; }
    finally { if (this.pending === pending) this.pending = undefined; }
  }
  private async fetch(block: number, signal: AbortSignal): Promise<Acquired> {
    // Fetch by the same height concurrently. Rust binds the proof's block hash
    // and global roots to this independently trusted header, including reorgs.
    const [header, proof] = await Promise.all([
      this.available(this.opts.rpcUrl, "starknet_getBlockWithTxHashes", [
        { block_number: block },
      ], signal),
      this.available(this.opts.proofRpcUrl, "starknet_getStorageProof",
        [{ block_number: block }, [], [hex(this.profile.pool)], []], signal),
      this.checkChain(signal),
    ]) as [{ block_number: number; block_hash: string; new_root: string;
      status: string }, unknown, void];
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
    return { checkpoint: cp, proof: JSON.stringify(proof) };
  }
  private async checkChain(signal: AbortSignal): Promise<void> {
    if (this.chainChecked) return;
    const id = await this.net.rpc(this.opts.rpcUrl, "starknet_chainId", [], signal);
    const expected = `0x${[...this.profile.chainId].map((c) => c.charCodeAt(0).toString(16)).join("")}`;
    if (typeof id !== "string" || hex(id) !== hex(expected))
      throw new Error("CHAIN_MISMATCH: checkpoint RPC");
    this.chainChecked = true;
  }
  private async available(
    url: string,
    method: "starknet_getBlockWithTxHashes" | "starknet_getStorageProof",
    params: unknown[],
    signal: AbortSignal,
  ): Promise<unknown> {
    for (let attempt = 0; ; attempt++) {
      try {
        return await this.net.rpc(url, method, params, signal);
      } catch (e) {
        // New-block availability is per backend behind the endpoint. A bounded
        // immediate retry avoids the demo's one-second error backoff; neither
        // this nor proof acquisition changes the independently trusted URL.
        const retryable = method === "starknet_getStorageProof"
          ? /RPC_UNAVAILABLE: (42|24|-32603)\b/ : /RPC_UNAVAILABLE: 24\b/;
        if (attempt === 2 || signal.aborted || !retryable.test(String(e))) throw e;
      }
    }
  }
}
