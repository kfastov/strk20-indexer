/**
 * The engine seam.
 *
 * `Engine` below is consumer-path.md §3.3's exported wasm ABI, transcribed
 * one-for-one. It is SYNCHRONOUS by contract: bytes in, notes out, no network,
 * no storage, no async inside the computer. Everything asynchronous — fetch,
 * IndexedDB, zstd, SSE — lives above this line, in TypeScript.
 *
 * Two implementations satisfy it:
 *   - `engine-wasm.ts` — the real one, calling the module built from
 *     `crates/strk20-engine`. Not available yet (the 0a refactor and the wasm
 *     build land later).
 *   - `engine-mock.ts` — a TypeScript stand-in that runs the SAME Step
 *     trampoline over a real static feed, with real fetches, real sha256
 *     verification and a real per-note trial scan.
 *
 * The demo switches between them by changing ONE binding
 * (`ts/demo/src/engine-binding.ts`) and nothing else.
 *
 * An `EngineFactory` carries a `kind`, a `label` and a `provenance` string, and
 * the client republishes all three through `status().engine`, so a screenshot
 * cannot misrepresent which computer produced a number.
 */

export interface DiscoverOut {
  /** strk20_consumer::sync::SyncReport, field-identical to `strk20-sync sync --json`. */
  report_json: string;
  /** Checkpoint-only sealed blob; hand it back next time. */
  sealed: Uint8Array;
  /** Notes not present in the supplied sealed blob. */
  added_json: string;
  /** Nullifiers that flipped to spent this pass. */
  spent_json: string;
  /** Counts only. Scanner-asserted key-clean like every other string emitted. */
  stats_json: string;
}

export interface Engine {
  /** §3.3's info() JSON. */
  info(): string;

  sync_begin(coldStart: 'auto' | 'snapshot' | 'epochs'): string;
  sync_supply(metaJson: string, compressed: Uint8Array | null, payload: Uint8Array | null): string;
  sync_supply_rpc(metaJson: string, resultJson: string | null): string;
  sync_abort(): void;

  /** Canonical NDJSON of every request this Engine has asked for, and its hash. */
  request_log(): string;
  request_log_sha256(): string;

  export_begin(): number;
  export_chunk(i: number): Uint8Array;
  export_end(): void;

  /**
   * `key` is copied into the module and the caller's staging buffer is zeroized
   * before return. `entropy32` MUST be 32 fresh bytes on EVERY call (§3.6).
   */
  discover_begin(
    ownerHex: string,
    key: Uint8Array,
    sealed: Uint8Array | null,
    entropy32: Uint8Array,
  ): number;
  discover_step(handle: number, maxOps: number): string;
  discover_finish(handle: number): DiscoverOut;
  discover_abort(handle: number): void;

  history(
    ownerHex: string,
    key: Uint8Array,
    sealed: Uint8Array | null,
    fromBlock: number | null,
    limit: number,
  ): string;

  export_reference_cursor(key: Uint8Array, sealed: Uint8Array): string;

  /** Release linear memory. `close()` on the client calls it. */
  free(): void;

  /** Linear memory currently held, or 0 when the implementation cannot report it. */
  memoryBytes(): number;
}

export interface EngineFactory {
  readonly kind: 'wasm' | 'mock';
  /** Shown verbatim in the demo's adapter badge. */
  readonly label: string;
  /** One sentence: what this engine's numbers include and exclude. */
  readonly provenance: string;
  create(profileJson: string): Promise<Engine>;
  /**
   * Restore from §3.5 frames. Returns null when the blob is unusable for any
   * reason the caller should treat as a cache miss (STATE_*), which is always
   * safe: deleting the folded blob is always correct.
   */
  load(profileJson: string, frames: readonly Uint8Array[]): Promise<Engine | null>;
}

// ---------------------------------------------------------------- Step shapes
// §3.3.1, byte-precise. These are the JSON the module emits; the wrapper parses
// them and does exactly what they say.

export interface StepFetch {
  step: 'fetch';
  seq: number;
  artifact: 'genesis' | 'manifest' | 'epoch' | 'epoch_anchor' | 'snapshot' | 'snapshot_anchor' | 'anchors' | 'head';
  path: string;
  optional: boolean;
  compressed: boolean;
  decompress_cap: number | null;
  /**
   * sha256 of the bytes AS SERVED, when the engine knows it from the manifest.
   * The wrapper must check it BEFORE inflating (crates/wasm/README.md, "What
   * TypeScript must do", point 2): the module re-hashes the inflated payload,
   * but nothing in Rust ever sees an epoch's `.zst`, so this check exists in
   * TypeScript or it does not exist. `null` means "no published hash for these
   * bytes" — genesis, manifest and head, which are not content-addressed.
   */
  sha256: string | null;
  conditional: { if_none_match: string } | null;
  reason: string;
  prefetch: {
    artifact: StepFetch['artifact'];
    path: string;
    compressed: boolean;
    decompress_cap: number | null;
    sha256: string | null;
  }[];
}

export interface StepRpc {
  step: 'rpc';
  seq: number;
  endpoint: 'anchor';
  method: string;
  params: unknown[];
  also?: { method: string; params: unknown[] }[];
  reason: string;
}

export interface StepDone {
  step: 'done';
  staleness: 'ok' | 'behind' | 'diverged';
  verified: 'anchored' | 'server-asserted' | 'replayed';
  state_dirty: boolean;
  outcome: {
    epochs_applied: number;
    tail_rewound: boolean;
    tail_changed: boolean;
    head: number;
    l1_accepted: number;
    last_epoch_to: number;
    snapshot_basis: number | null;
    snapshot_rejected: boolean;
    history_floor: number;
  };
}

export type Step = StepFetch | StepRpc | StepDone;

export interface ResponseEnvelope {
  seq: number;
  status: number;
  not_modified: boolean;
  absent: boolean;
  etag: string | null;
}

export interface EngineInfo {
  chain_id: string;
  pool: string;
  genesis_block: number;
  epoch_size: number;
  last_epoch: number;
  last_epoch_hash: string;
  last_epoch_to: number;
  history_floor: number;
  snapshot_basis: number | null;
  snapshot_pending_grounding: boolean;
  head: number;
  l1_accepted: number;
  slots: number;
  blocks: number;
  events: number;
  verified: 'anchored' | 'server-asserted' | 'replayed';
  engine_version: string;
  state_dirty: boolean;
}

export interface DiscoverStepOut {
  done: boolean;
  phase: 'ckpt_in' | 'ckpt_out' | 'live_in' | 'live_out' | 'spent' | 'done';
  ops: number;
  ops_total: number;
  channels: number;
  notes: number;
}
