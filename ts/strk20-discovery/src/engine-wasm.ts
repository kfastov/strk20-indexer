/**
 * The REAL engine adapter: `crates/wasm` (npm name `strk20-engine`), which is
 * `crates/consumer` compiled to `wasm32-unknown-unknown` behind a wasm-bindgen
 * facade. Same code as the native client — its smoke test folds a real feed
 * natively and through the module and demands the two `SyncReport`s be
 * byte-identical.
 *
 * ---------------------------------------------------------------------------
 * WHY THIS FILE IS NOT THIN, AND WHAT THAT MEANS
 * ---------------------------------------------------------------------------
 *
 * `engine.ts` transcribes consumer-path.md §3.3: a module that OWNS the fetch
 * plan and emits `Step::Fetch` for the wrapper to satisfy. The shipped module
 * does not have that ABI and deliberately never will — `crates/wasm/README.md`
 * §"Where §A3 turned out to be wrong" records why: `strk20-consumer` has no
 * byte-level apply, only `apply_feed(store, transport, cold_start)`, one
 * incremental pass that PULLS. So the module ships `stage_*` + `apply`
 * instead, and there is no `history()` and no `export_reference_cursor()`.
 *
 * This adapter is therefore the trampoline: it authors the fetch plan from the
 * manifest, drives it through the §3.3 Step JSON the client already knows how
 * to satisfy, and stages the returned bytes into the module.
 *
 * The honest consequence, stated because a demo that hides it is a demo that
 * lies: **TypeScript authors the URLs here, not Rust.** §4.10's claim that
 * "the component that authors the URLs cannot see a key" still holds — the
 * plan below is a pure function of `manifest.json` and the module's own
 * `info()`, and no branch of it can observe a viewing key, which is why the
 * A/B run still produces identical request logs for two identities. But the
 * claim is now enforced by this file rather than by the module's own
 * `request_log()`, which does not exist. `request_log_sha256()` below is
 * computed HERE and labelled as such in `provenance`.
 *
 * What Rust still owns, unchanged and un-reimplemented: the epoch hash chain,
 * the chain/pool binding, the §1.5 snapshot ladder, reachability grounding,
 * reorg supersede, the trial-decryption scan and the report. TypeScript never
 * verifies anything it then reports as verified.
 *
 * ---------------------------------------------------------------------------
 * KEY HANDLING
 * ---------------------------------------------------------------------------
 *
 * The whole discovery pass runs inside `discover_begin`, synchronously. That
 * is not laziness about the step API — it is the only shape in which the key
 * never outlives one synchronous call. The adapter copies the caller's bytes,
 * hands the copy to `Engine.discover`, which zeroizes it in place, and
 * zeroizes the caller's buffer too. Nothing between `begin` and `finish` holds
 * key material.
 */

import { Strk20Error } from './errors.ts';
import { feltEq } from './felt.ts';
import { sha256Hex } from './sha256.ts';
import type {
  DiscoverOut,
  DiscoverStepOut,
  Engine,
  EngineFactory,
  EngineInfo,
  ResponseEnvelope,
  StepDone,
  StepFetch,
} from './engine.ts';
import type { ChainProfile } from './types.ts';

// --------------------------------------------------------------- the module

/** `crates/wasm/pkg/strk20_engine.d.ts`, exactly. */
interface WasmEngine {
  apply(coldStart: string): string;
  apply_head(payload: Uint8Array, etag: string): string;
  check_manifest(manifestJson: string): string;
  clear_storage_proofs(): void;
  discover(ownerHex: string, key: Uint8Array): string;
  export_state(): Uint8Array;
  forget_owner(ownerHex: string): void;
  info(): string;
  proof_candidates(): string;
  stage_anchors(payload: Uint8Array): void;
  stage_epoch(e: bigint, payload: Uint8Array): void;
  stage_head(payload: Uint8Array, etag: string): void;
  stage_manifest(manifestJson: string): void;
  stage_snapshot(e: bigint, zst: Uint8Array, payload: Uint8Array): void;
  stage_snapshot_anchor(e: bigint, json: Uint8Array): void;
  stage_storage_proof(block: bigint, proofJson: string): void;
  free(): void;
}

interface WasmEngineClass {
  new (genesisJson: string): WasmEngine;
  load(blob: Uint8Array, genesisJson: string): WasmEngine;
  version(): string;
}

/** The wasm-pack `--target web` module namespace. */
export interface WasmGlue {
  default: (init?: unknown) => Promise<{ memory: WebAssembly.Memory }>;
  Engine: WasmEngineClass;
  set_panic_hook: () => void;
}

export interface WasmEngineOptions {
  /**
   * Supplies the wasm-pack glue. Injected rather than imported because §4.10's
   * chokepoint scan forbids a dynamic `import()` of a URL anywhere in `src/`
   * except `net.ts` — the host does the import, this package never does.
   */
  loadGlue: () => Promise<WasmGlue>;
  /**
   * Overrides the glue's default `new URL('strk20_engine_bg.wasm',
   * import.meta.url)`. Bytes are accepted too, which is how Node loads the
   * SAME artifact the browser does rather than a second build.
   */
  wasmUrl?: string | URL | BufferSource | WebAssembly.Module;
}

// ------------------------------------------------------------- feed shapes

interface ManifestEpoch {
  e: number;
  from: number;
  to: number;
  /** sha256 of the UNCOMPRESSED payload — the module re-checks this. */
  hash: string;
  /** sha256 of the `.zst` file. Checked in TypeScript, before inflating. */
  zst: string;
  bytes: number;
}

interface ManifestSnapshot {
  e: number;
  block: number;
  file: string;
  hash: string;
  zst: string;
  bytes: number;
}

interface Manifest {
  v: number;
  chain_id: string;
  pool: string;
  genesis_block: number;
  epoch_size: number;
  head: { number: number; hash: string; l1_accepted: number };
  latest_epoch: number | null;
  epochs: ManifestEpoch[];
  /**
   * OPTIONAL, not `| null`. A feed with no snapshot may write `null` or may
   * omit the key entirely, and this said `snapshot: ManifestSnapshot | null`
   * while the planner tested `!== null` — so the omitted case type-checked its
   * way into dereferencing `undefined`.
   */
  snapshot?: ManifestSnapshot | null;
}

/** What `apply` returns. */
interface Applied {
  epochs_applied: number;
  last_epoch: number | null;
  last_epoch_to: number;
  head: number;
  l1_accepted: number;
  tail_rewound: boolean;
  history_floor: number;
  snapshot_basis: number | null;
  snapshot_rejected: boolean;
  state_changed: boolean;
}

/** What the module's own `info()` returns (note `last_epoch: number | null`). */
interface RawInfo {
  chain_id: string;
  pool: string;
  genesis_block: number;
  epoch_size: number;
  last_epoch: number | null;
  last_epoch_hash: string | null;
  last_epoch_to: number;
  history_floor: number;
  snapshot_basis: number | null;
  head: number;
  l1_accepted: number;
  slots: number;
  tail_generation: number;
  /** The trust grade, DECIDED BY THE MODULE. Never re-derived here. */
  verified: 'anchored' | 'server-asserted' | 'replayed';
  engine_version: string;
}

interface ReportNote {
  token: string;
  index: number;
  note_id: string;
  nullifier: string;
  amount: string;
  block_number: number;
  sender: string;
  spent: boolean;
}

interface SyncReport {
  address: string;
  notes: ReportNote[];
  newly_spent: string[];
  verified: 'anchored' | 'server-asserted' | 'replayed';
  history_from: number;
  snapshot_basis: number | null;
}

// ------------------------------------------------------------- the planner

/** One queued fetch, before it becomes a numbered Step. */
interface Planned {
  artifact: StepFetch['artifact'];
  path: string;
  optional: boolean;
  compressed: boolean;
  /** sha256 of the bytes as served. Verified BEFORE inflating (README §2). */
  sha256: string | null;
  reason: string;
  /** For epochs and snapshots: which one, so `stage_*` can be called. */
  e?: number;
}

const EPOCH_CAP = 64 * 2 ** 20;
const SNAPSHOT_CAP = 512 * 2 ** 20;
/** IndexedDB is happier with several medium values than one enormous one. */
const FRAME_BYTES = 4 * 2 ** 20;

function epochPath(e: number): string {
  return `/epochs/${String(e).padStart(8, '0')}.strk20e.zst`;
}

const dec = new TextDecoder();
const enc = new TextEncoder();

// ------------------------------------------------------------- the adapter

class WasmEngineAdapter implements Engine {
  readonly #glue: WasmGlue;
  readonly #memory: WebAssembly.Memory;
  #inner: WasmEngine | null;
  #genesisJson: string | null;
  /** Built by `Engine.load` from a state blob, so its artifacts are staged. */
  readonly #restored: boolean;
  /**
   * The chain identity the CALLER pinned, before a byte was requested. Checked
   * against the feed's own `genesis.json` in `sync_supply`, which is what
   * closes trust-on-first-use (§3.10 item 3): an empty mirror must not adopt
   * whatever chain the feed declares — and this engine takes its feed base
   * from a URL parameter in the demo.
   */
  readonly #profile: ChainProfile;

  // -- sync state
  #manifestJson: string | null = null;
  #manifest: Manifest | null = null;
  #queue: Planned[] = [];
  #pending: (Planned & { seq: number }) | null = null;
  #seq = 0;
  #coldStart: 'auto' | 'snapshot' | 'epochs' = 'auto';
  #applied: Applied | null = null;
  #headEtag: string | null = null;
  #tailChanged = false;
  /** Set when the plan has been drained once and a catch-up round may follow. */
  #round = 0;
  #log: string[] = [];
  #exportFrames: Uint8Array[] | null = null;

  // -- discovery state
  #handles = new Map<number, { report: SyncReport; json: string }>();
  #nextHandle = 1;
  /** Previous report per owner, so `added`/`spent` are a real diff. */
  #seen = new Map<string, { notes: Set<string>; spent: Set<string> }>();

  constructor(
    glue: WasmGlue,
    memory: WebAssembly.Memory,
    inner: WasmEngine | null,
    genesisJson: string | null,
    profile: ChainProfile,
  ) {
    this.#glue = glue;
    this.#memory = memory;
    this.#inner = inner;
    this.#genesisJson = genesisJson;
    this.#restored = inner !== null;
    this.#profile = profile;
  }

  #call<T>(f: () => T): T {
    try {
      return f();
    } catch (e) {
      throw Strk20Error.fromModuleJson(e);
    }
  }

  #engine(): WasmEngine {
    if (!this.#inner) {
      throw new Strk20Error('SYNC_PROTOCOL', 'the engine has no genesis yet; sync_begin fetches it first');
    }
    return this.#inner;
  }

  #info(): RawInfo | null {
    return this.#inner ? (JSON.parse(this.#inner.info()) as RawInfo) : null;
  }

  // ------------------------------------------------------------------ info

  info(): string {
    // ONE module round trip. This used to parse `info()` twice — once here and
    // once inside `#grade()` — and each parse made the module recompute the
    // `slots` field, which is a full scan of the storage log.
    const raw = this.#info();
    const verified = raw?.verified ?? 'replayed';
    // `last_epoch` is `null` in the module when nothing is folded. The client
    // tests `last_epoch < 0` for coldness, and `null < 0` is false in JS —
    // which would report a cold engine as warm. Normalised here, once.
    const out: EngineInfo = {
      chain_id: raw?.chain_id ?? '',
      pool: raw?.pool ?? '',
      genesis_block: raw?.genesis_block ?? 0,
      epoch_size: raw?.epoch_size ?? 0,
      last_epoch: raw?.last_epoch ?? -1,
      last_epoch_hash: raw?.last_epoch_hash ?? '',
      last_epoch_to: raw?.last_epoch_to ?? 0,
      history_floor: raw?.history_floor ?? 0,
      snapshot_basis: raw?.snapshot_basis ?? null,
      snapshot_pending_grounding: false,
      head: raw?.head ?? 0,
      l1_accepted: raw?.l1_accepted ?? 0,
      slots: raw?.slots ?? 0,
      // The module's mirror exposes no block or event count — `ConsumerStore`
      // cannot enumerate events at all (blob.rs). -1 is the adapter's
      // "unmeasured", never a zero passed off as a measurement.
      blocks: -1,
      events: -1,
      verified,
      engine_version: raw?.engine_version ?? '',
      state_dirty: this.#applied?.state_changed ?? false,
    };
    return JSON.stringify(out);
  }

  /**
   * The grade the mirror has EARNED — **read from the module**, not computed
   * here.
   *
   * It used to be computed here, as `snapshot_basis === null ? 'replayed' :
   * 'server-asserted'`, which made `'anchored'` unreachable by construction: no
   * input to this adapter could ever produce it, while the demo displayed the
   * result labelled `provenance: 'measured'`. Two copies of a trust rule is one
   * copy too many, and the copy that could not express the strongest grade was
   * the one on screen. `Engine::info()` now carries `verified` and this reads
   * it.
   *
   * Note what this does NOT mean: nothing in `ts/` stages a storage proof, so
   * the module will not report `'anchored'` on this path. That is a wiring gap
   * (see `sync_supply_rpc`), and the honest way to show it is to relay the
   * module's own answer rather than to hard-code the same conclusion here.
   */
  #grade(): 'anchored' | 'server-asserted' | 'replayed' {
    return this.#info()?.verified ?? 'replayed';
  }

  // ------------------------------------------------------------------ sync

  sync_begin(coldStart: 'auto' | 'snapshot' | 'epochs'): string {
    this.#coldStart = coldStart;
    this.#queue = [];
    this.#pending = null;
    this.#seq = 0;
    this.#round = 0;
    this.#applied = null;
    this.#tailChanged = false;
    this.#log = [];

    if (!this.#inner) {
      this.#queue.push({
        artifact: 'genesis',
        path: '/genesis.json',
        optional: false,
        compressed: false,
        sha256: null,
        reason: 'pin chain identity before anything else is believed',
      });
    }
    this.#queue.push({
      artifact: 'manifest',
      path: '/manifest.json',
      optional: false,
      compressed: false,
      sha256: null,
      reason: 'the epoch inventory and the head this run is aiming at',
    });
    return this.#next();
  }

  sync_supply(metaJson: string, compressed: Uint8Array | null, payload: Uint8Array | null): string {
    const env = JSON.parse(metaJson) as ResponseEnvelope;
    const step = this.#pending;
    if (!step || step.seq !== env.seq) {
      throw new Strk20Error('SYNC_PROTOCOL', 'a response arrived for a step that was not outstanding', {
        got: env.seq,
        expected: step?.seq ?? null,
      });
    }
    this.#pending = null;

    if (env.absent) {
      if (!step.optional) {
        throw new Strk20Error('FEED_MALFORMED', 'a required feed artifact is absent', { path: step.path });
      }
      return this.#next();
    }

    // The bytes exactly as served. For a compressed artifact this is the
    // `.zst`; `payload` is what the client inflated from it.
    const served = compressed;
    if (!env.not_modified && !served) {
      throw new Strk20Error('TRANSPORT', 'the transport returned no bytes for a required artifact', {
        path: step.path,
        status: env.status,
      });
    }

    this.#log.push(`GET ${step.path}`);

    switch (step.artifact) {
      case 'genesis': {
        const json = dec.decode(served!);
        // BEFORE the engine exists. Everything downstream is checked against
        // this document by the module, so a genesis that is not ours makes the
        // whole fold a self-consistent fold of somebody else's chain — folded
        // correctly, reported `replayed`, and about nothing the caller asked
        // for. §3.10 item 3.
        checkGenesisAgainstProfile(json, this.#profile);
        this.#genesisJson = json;
        this.#inner = this.#call(() => new this.#glue.Engine(this.#genesisJson!));
        break;
      }
      case 'manifest': {
        this.#manifestJson = dec.decode(served!);
        this.#manifest = JSON.parse(this.#manifestJson) as Manifest;
        this.#call(() => this.#engine().stage_manifest(this.#manifestJson!));
        this.#planFromManifest();
        break;
      }
      case 'epoch': {
        // Rings: TypeScript checked the `.zst` hash before inflating (client),
        // the module re-hashes the inflated payload and checks its header
        // range and prev-linkage inside `apply`.
        this.#call(() => this.#engine().stage_epoch(BigInt(step.e!), payload!));
        break;
      }
      case 'snapshot': {
        this.#call(() => this.#engine().stage_snapshot(BigInt(step.e!), served!, payload!));
        break;
      }
      case 'snapshot_anchor': {
        this.#call(() => this.#engine().stage_snapshot_anchor(BigInt(step.e!), served!));
        break;
      }
      case 'anchors': {
        this.#call(() => this.#engine().stage_anchors(served!));
        break;
      }
      case 'head': {
        const etag = env.etag ?? '';
        this.#tailChanged = etag === '' || etag !== this.#headEtag;
        this.#headEtag = etag;
        if (!env.not_modified) this.#call(() => this.#engine().stage_head(payload ?? served!, etag));
        break;
      }
      default:
        throw new Strk20Error('SYNC_PROTOCOL', 'unplanned artifact', { artifact: step.artifact });
    }
    return this.#next();
  }

  sync_supply_rpc(metaJson: string, _resultJson: string | null): string {
    // This adapter never emits a Step::Rpc, so nothing should arrive here. §1.5
    // ring 6 (the `anchored` grade) needs `starknet_getStorageProof` against
    // the USER's own node; the module supports it via `stage_storage_proof`,
    // and the wiring for it is a client option this build does not accept.
    void metaJson;
    throw new Strk20Error('SYNC_PROTOCOL', 'this engine emits no RPC step; ring 6 is not wired in this build');
  }

  sync_abort(): void {
    this.#queue = [];
    this.#pending = null;
  }

  /**
   * The whole fetch plan, derived from `manifest.json` and the module's own
   * `info()`. Key-blind by construction: no argument, field or branch below
   * can observe an owner or a viewing key.
   */
  #planFromManifest(): void {
    const m = this.#manifest!;
    const info = this.#info()!;

    // A RESTORED engine cannot be planned for yet. `Engine.load` re-stages
    // bytes without folding them, so `info()` still reports `last_epoch: null`
    // and an epoch plan built here would ask for all 607 again — served from
    // IndexedDB, but re-read, re-inflated and re-staged for nothing.
    //
    // So fetch only the head, fold what the blob already carries, and let
    // `#fold`'s catch-up round plan the epochs the manifest has ABOVE that.
    // The head must be staged before that first fold rather than after: with
    // no tail the mirror's head is 0, the §11.3 reachability walk finds no
    // anchor at or below it, and a snapshot-started mirror would be rejected
    // as ungrounded and silently fall back to a full replay.
    if (this.#restored && info.last_epoch === null) {
      this.#queue.push({
        artifact: 'head',
        path: '/head.ndjson',
        optional: false,
        compressed: false,
        sha256: null,
        reason: 'the tail, before folding what the restored blob carries',
      });
      return;
    }

    const last = info.last_epoch ?? -1;

    // `!m.snapshot`, not `m.snapshot !== null`. A feed that publishes no
    // snapshot writes `"snapshot": null` OR simply omits the key, and both of
    // ours have done both — `undefined !== null` is true, so the omitted case
    // planned a snapshot fetch and then dereferenced `m.snapshot!.file` two
    // statements later. That threw a bare TypeError out of the adapter, which
    // is not a member of §4.2's error union and defeated every `catch`
    // downstream. The line below already had the falsy check; this one did not.
    const wantSnapshot =
      !!m.snapshot && last < 0 && this.#coldStart !== 'epochs' && info.snapshot_basis === null;

    if (this.#coldStart === 'snapshot' && !m.snapshot) {
      throw new Strk20Error('SNAPSHOT_UNAVAILABLE', 'coldStart:"snapshot" was demanded but this feed publishes none');
    }

    if (wantSnapshot) {
      const s = m.snapshot!;
      this.#queue.push({
        artifact: 'snapshot',
        path: `/${s.file.replace(/^\//, '')}`,
        optional: false,
        compressed: true,
        sha256: s.zst,
        reason: 'cold start from the published snapshot rather than replaying every epoch',
        e: s.e,
      });
      this.#queue.push({
        artifact: 'snapshot_anchor',
        path: `/snapshots/${String(s.e).padStart(8, '0')}.anchor.json`,
        optional: true,
        compressed: false,
        sha256: null,
        reason: 'the §12 basis anchor sidecar, when the feed publishes one',
        e: s.e,
      });
      this.#queue.push({
        artifact: 'anchors',
        path: '/anchors.ndjson',
        optional: false,
        compressed: false,
        sha256: null,
        reason: 'the §11.3 reachability walk is what grounds a snapshot slot set',
      });
    }

    // Every epoch the mirror has not folded. `apply` skips already-applied
    // ones, but re-fetching them would be a wasted GET, so they are not asked
    // for at all.
    const from = wantSnapshot ? (m.snapshot!.e ?? -1) : last;
    for (const ep of m.epochs) {
      if (ep.e <= from) continue;
      this.#queue.push({
        artifact: 'epoch',
        path: epochPath(ep.e),
        optional: false,
        compressed: true,
        sha256: ep.zst,
        reason: `epoch ${ep.e} (blocks ${ep.from}–${ep.to})`,
        e: ep.e,
      });
    }

    this.#queue.push({
      artifact: 'head',
      path: '/head.ndjson',
      optional: false,
      compressed: false,
      sha256: null,
      reason: 'the unfinalised tail above the last cut epoch',
    });
  }

  /** Emit the next Step, or fold and finish. */
  #next(): string {
    const item = this.#queue.shift();
    if (item) {
      const seq = this.#seq++;
      this.#pending = { ...item, seq };

      // Prefetch hints: the client fires these in parallel beside the primary
      // GET. 606 sequential epoch round trips is the difference between a
      // demo and a coffee break.
      const prefetch = this.#queue.slice(0, 24).map((p) => ({
        artifact: p.artifact,
        path: p.path,
        compressed: p.compressed,
        decompress_cap: capOf(p),
        sha256: p.sha256,
      }));

      const step: StepFetch = {
        step: 'fetch',
        seq,
        artifact: item.artifact,
        path: item.path,
        optional: item.optional,
        compressed: item.compressed,
        decompress_cap: capOf(item),
        sha256: item.sha256,
        conditional: item.artifact === 'head' && this.#headEtag ? { if_none_match: this.#headEtag } : null,
        reason: item.reason,
        prefetch,
      };
      return JSON.stringify(step);
    }
    return this.#fold();
  }

  /**
   * Everything staged: fold it. A restored engine needs this before its
   * `info()` says anything true — `load` restores BYTES, not a folded mirror —
   * so the first fold may reveal epochs the plan did not know to ask for. That
   * is round 2, and there is never a round 3: after one fold `info()` is
   * authoritative and the manifest is fixed for this pass.
   */
  #fold(): string {
    const e = this.#engine();
    const applied = JSON.parse(this.#call(() => e.apply(this.#coldStart))) as Applied;
    this.#applied = applied;

    if (this.#round === 0) {
      this.#round = 1;
      const before = this.#queue.length;
      this.#planCatchUp();
      if (this.#queue.length > before) return this.#next();
    }

    const staleness = this.#manifestJson
      ? (this.#call(() => e.check_manifest(this.#manifestJson!)) as 'ok' | 'behind' | 'diverged')
      : 'ok';

    const done: StepDone = {
      step: 'done',
      staleness,
      verified: this.#grade(),
      state_dirty: applied.state_changed,
      outcome: {
        epochs_applied: applied.epochs_applied,
        tail_rewound: applied.tail_rewound,
        tail_changed: this.#tailChanged || applied.tail_rewound,
        head: applied.head,
        l1_accepted: applied.l1_accepted,
        last_epoch_to: applied.last_epoch_to,
        snapshot_basis: applied.snapshot_basis,
        snapshot_rejected: applied.snapshot_rejected,
        history_floor: applied.history_floor,
      },
    };
    return JSON.stringify(done);
  }

  /** Epochs the manifest lists above what the first fold actually reached. */
  #planCatchUp(): void {
    const m = this.#manifest;
    if (!m) return;
    const last = this.#info()?.last_epoch ?? -1;
    let added = false;
    for (const ep of m.epochs) {
      if (ep.e <= last) continue;
      added = true;
      this.#queue.push({
        artifact: 'epoch',
        path: epochPath(ep.e),
        optional: false,
        compressed: true,
        sha256: ep.zst,
        reason: `epoch ${ep.e}, above the restored blob's floor`,
        e: ep.e,
      });
    }
    // A second head is only worth a round trip if epochs moved under it.
    if (added) {
      this.#queue.push({
        artifact: 'head',
        path: '/head.ndjson',
        optional: false,
        compressed: false,
        sha256: null,
        reason: 'the tail above the newly folded epochs',
      });
    }
  }

  // ------------------------------------------------------------ request log

  request_log(): string {
    return this.#log.map((l) => JSON.stringify({ req: l })).join('\n');
  }

  request_log_sha256(): string {
    return sha256Hex(enc.encode(this.request_log()));
  }

  // ------------------------------------------------------------ persistence

  export_begin(): number {
    const blob = this.#call(() => this.#engine().export_state());
    // Frame 0 carries `genesis.json`, because `Engine.load` needs it and the
    // client's storage schema has nowhere else to put it. Frames 1..n are the
    // blob, chunked so IndexedDB is not asked to hold one 20 MB value.
    const frames: Uint8Array[] = [enc.encode(this.#genesisJson ?? '')];
    for (let off = 0; off < blob.length; off += FRAME_BYTES) {
      // `slice`, NOT `subarray`. A subarray is a view onto the whole blob, and
      // IndexedDB's structured clone copies the view's entire backing buffer —
      // so three 4 MB "frames" over one 10 MB blob would store 30 MB and read
      // 30 MB back. `slice` copies once here so each frame owns only its own
      // bytes.
      frames.push(blob.slice(off, Math.min(off + FRAME_BYTES, blob.length)));
    }
    this.#exportFrames = frames;
    return frames.length;
  }

  export_chunk(i: number): Uint8Array {
    const f = this.#exportFrames?.[i];
    if (!f) throw new Strk20Error('SYNC_PROTOCOL', 'export_chunk outside an export', { index: i });
    return f;
  }

  export_end(): void {
    this.#exportFrames = null;
  }

  // -------------------------------------------------------------- discovery

  discover_begin(ownerHex: string, key: Uint8Array, _sealed: Uint8Array | null, _entropy32: Uint8Array): number {
    // The module has no sealed-state ABI (§3.6 was not built — see
    // crates/wasm/README.md). The cursor lives in the module's in-memory store
    // and is not exportable, so a supplied blob is not merely ignored, there is
    // nothing it could be handed to. `entropy32` likewise has no consumer: the
    // module imports no randomness source at all (its import audit proves it).
    if (key.length !== 32) {
      throw new Strk20Error('KEY_INVALID', 'the viewing key must be exactly 32 bytes', { got: key.length });
    }
    // One synchronous window: copy in, Rust zeroizes the copy, we zeroize the
    // caller's buffer. No key material survives this call.
    const scratch = Uint8Array.from(key);
    let json: string;
    try {
      json = this.#call(() => this.#engine().discover(ownerHex, scratch));
    } finally {
      scratch.fill(0);
      key.fill(0);
    }
    const handle = this.#nextHandle++;
    this.#handles.set(handle, { report: JSON.parse(json) as SyncReport, json });
    return handle;
  }

  discover_step(handle: number, _maxOps: number): string {
    // The module's `discover` is one indivisible call — `sync_once` over the
    // whole mirror — so there is nothing to slice. Reporting `done` on the
    // first step is the truth; a fake progress ramp would not be.
    const h = this.#handles.get(handle);
    if (!h) throw new Strk20Error('SESSION_INVALID', 'no such discovery handle', { handle });
    const out: DiscoverStepOut = {
      done: true,
      phase: 'done',
      ops: 1,
      ops_total: 1,
      channels: 0,
      notes: h.report.notes.length,
    };
    return JSON.stringify(out);
  }

  discover_finish(handle: number): DiscoverOut {
    const h = this.#handles.get(handle);
    if (!h) throw new Strk20Error('SESSION_INVALID', 'no such discovery handle', { handle });
    this.#handles.delete(handle);

    // `added` / `spent` are a real diff against this adapter's previous report
    // for the same owner. The module does not compute them (no sealed cursor),
    // and the first pass legitimately reports every note as added.
    const key = h.report.address;
    const prev = this.#seen.get(key);
    const notes = h.report.notes;
    const added = prev ? notes.filter((n) => !prev.notes.has(n.note_id)) : notes;
    const spent = prev
      ? notes.filter((n) => n.spent && !prev.spent.has(n.note_id))
      : notes.filter((n) => n.spent);
    this.#seen.set(key, {
      notes: new Set(notes.map((n) => n.note_id)),
      spent: new Set(notes.filter((n) => n.spent).map((n) => n.note_id)),
    });

    return {
      report_json: h.json,
      // No sealed cursor exists to hand back. An empty blob is honest; the
      // client stores it and `discover_begin` ignores what it gets.
      sealed: new Uint8Array(0),
      added_json: JSON.stringify(added.map(toClientNote)),
      spent_json: JSON.stringify(spent.map(toClientNote)),
      stats_json: JSON.stringify({
        slots_read: this.#info()?.slots ?? -1,
        // The module exposes no scan counter. -1 is "not measured"; a 0 here
        // would be a fabricated measurement on a screen that labels it one.
        events_scanned: -1,
        passes_in: 1,
        passes_out: 1,
        cursor_reset: false,
      }),
    };
  }

  discover_abort(handle: number): void {
    this.#handles.delete(handle);
  }

  history(): string {
    // Block B has no history API — crates/wasm/README.md lists `history()`
    // under "Not built". A stub returning an empty list would read as "this
    // account has no transactions", which is a different and false claim.
    throw new Strk20Error('HISTORY_UNAVAILABLE', 'the wasm engine exposes no transaction history API');
  }

  export_reference_cursor(): string {
    throw new Strk20Error('SESSION_INCOMPLETE', 'no sealed cursor exists to export from this engine');
  }

  free(): void {
    this.#inner?.free();
    this.#inner = null;
  }

  memoryBytes(): number {
    return this.#memory.buffer.byteLength;
  }
}

// ------------------------------------------------------------------ helpers

/** `genesis.json` as the feed publishes it — the four fields that are identity. */
interface GenesisDoc {
  chain_id?: unknown;
  pool?: unknown;
  genesis_block?: unknown;
  epoch_size?: unknown;
}

/**
 * The trust-on-first-use gate, ported from `engine-mock.ts` (which had it and
 * this adapter did not, on the engine that ships by default and whose feed base
 * comes from a `?feed=` URL parameter).
 *
 * Identity first, then geometry — a feed that agrees about the chain and the
 * pool but not about where epochs start is still not the feed this caller
 * pinned, and folding it would produce a mirror indexed against different block
 * boundaries.
 */
export function checkGenesisAgainstProfile(genesisJson: string, profile: ChainProfile): void {
  let g: GenesisDoc;
  try {
    g = JSON.parse(genesisJson) as GenesisDoc;
  } catch {
    throw new Strk20Error('FEED_MALFORMED', 'genesis.json is not JSON');
  }
  if (g.chain_id !== profile.chainId || !feltEq(g.pool, profile.pool)) {
    throw new Strk20Error('CHAIN_MISMATCH', 'feed genesis does not match the expected chain profile', {
      expected_chain_id: profile.chainId,
      got_chain_id: String(g.chain_id ?? ''),
      expected_pool: profile.pool,
      got_pool: String(g.pool ?? ''),
    });
  }
  if (g.genesis_block !== profile.genesisBlock || g.epoch_size !== profile.epochSize) {
    throw new Strk20Error('CHAIN_MISMATCH', 'feed genesis disagrees with the profile geometry', {
      expected_genesis_block: profile.genesisBlock,
      got_genesis_block: Number(g.genesis_block ?? -1),
      expected_epoch_size: profile.epochSize,
      got_epoch_size: Number(g.epoch_size ?? -1),
    });
  }
}

// ------------------------------------------------------- the error boundary

/**
 * §4.2's error union is CLOSED, and a closed union is a promise about what
 * comes OUT, not only about what this file chooses to throw. An integrator
 * writes `catch (e) { if (isStrk20Error(e)) report(e.code) }` and there is no
 * correct `else` branch to write — so a `TypeError` escaping from here is not
 * an ugly message, it is a broken contract that lands in the branch the
 * integrator was told could not happen.
 *
 * Two sources, one exit:
 *   - the module throws canonical JSON (§3.7), which round-trips with its own
 *     code and details;
 *   - anything else is a fault in THIS file — a manifest field the planner
 *     assumed was there, an arithmetic slip — and gets `INTERNAL`, carrying
 *     the original message verbatim so it is still debuggable rather than
 *     laundered into "something went wrong".
 */
function asStrk20Error(e: unknown): Strk20Error {
  if (e instanceof Strk20Error) return e;
  const converted = Strk20Error.fromModuleJson(e);
  if (converted.code !== 'INTERNAL') return converted;
  return new Strk20Error('INTERNAL', converted.message, { thrown: e instanceof Error ? e.name : typeof e });
}

/**
 * The boundary itself: every `Engine` method, converted on the way out.
 *
 * A Proxy rather than a try/catch repeated in each of the twenty methods,
 * because the guarantee has to hold for the method somebody adds next year
 * too, and a per-method wrapper is exactly the kind of list that gets one
 * entry short. `apply(target, …)` keeps `this` bound to the real adapter, so
 * its `#private` fields are untouched by the indirection.
 *
 * Every `Engine` method is synchronous by contract (`engine.ts`), so a
 * try/catch really does see everything; there is no promise to also catch.
 */
function guarded(engine: Engine): Engine {
  return new Proxy(engine, {
    get(target, prop) {
      const v = Reflect.get(target, prop) as unknown;
      if (typeof v !== 'function') return v;
      const fn = v as (...a: unknown[]) => unknown;
      return (...args: unknown[]): unknown => {
        try {
          return fn.apply(target, args);
        } catch (e) {
          throw asStrk20Error(e);
        }
      };
    },
  });
}

function capOf(p: Planned): number | null {
  if (!p.compressed) return null;
  return p.artifact === 'snapshot' ? SNAPSHOT_CAP : EPOCH_CAP;
}

/** `ReportNote` is snake_case; the client's `RawNote` is camelCase. */
function toClientNote(n: ReportNote): Record<string, unknown> {
  return {
    token: n.token,
    index: n.index,
    noteId: n.note_id,
    nullifier: n.nullifier,
    amount: n.amount,
    blockNumber: n.block_number,
    blockTimestamp: 0,
    sender: n.sender,
    spent: n.spent,
  };
}

// ------------------------------------------------------------------ factory

export function wasmEngineFactory(opts: WasmEngineOptions): EngineFactory {
  let booted: Promise<{ glue: WasmGlue; memory: WebAssembly.Memory }> | null = null;

  const boot = async () => {
    booted ??= (async () => {
      if (!opts.loadGlue) {
        throw new Strk20Error('CONFIG_INVALID', 'wasmEngineFactory needs loadGlue', { option: 'loadGlue' });
      }
      const glue = await opts.loadGlue();
      const out = await glue.default(opts.wasmUrl ? { module_or_path: opts.wasmUrl } : undefined);
      glue.set_panic_hook();
      return { glue, memory: out.memory };
    })();
    return booted;
  };

  /**
   * The body of `load`, so the boundary conversion below wraps it whole. The
   * inner `catch`es here are policy — §4.5's "a bad cache row is a cache miss"
   * — and they stay where they are; the outer one only ensures that whatever
   * they re-throw, and whatever `JSON.parse` throws before them, leaves as a
   * `Strk20Error`.
   */
  const loadFrames = async (profileJson: string, frames: readonly Uint8Array[]): Promise<Engine | null> => {
    const profile = JSON.parse(profileJson) as ChainProfile;
    const { glue, memory } = await boot();
    if (frames.length < 2) return null;
    const genesisJson = dec.decode(frames[0]!);
    // A persisted blob is a cache, and a cache is not evidence. The genesis
    // it carries is checked against the pinned profile exactly as a freshly
    // fetched one would be — otherwise a poisoned or stale IndexedDB row
    // reintroduces on reload the identity hole the network path closes.
    try {
      checkGenesisAgainstProfile(genesisJson, profile);
    } catch {
      return null; // §4.5: a foreign blob is a cache miss, and deleting it is safe.
    }
    const total = frames.slice(1).reduce((n, f) => n + f.length, 0);
    const blob = new Uint8Array(total);
    let off = 0;
    for (const f of frames.slice(1)) {
      blob.set(f, off);
      off += f.length;
    }
    try {
      const inner = glue.Engine.load(blob, genesisJson);
      return guarded(new WasmEngineAdapter(glue, memory, inner, genesisJson, profile));
    } catch (e) {
      const err = Strk20Error.fromModuleJson(e);
      // §4.5: a blob that will not load is a cache miss, and deleting it is
      // always safe. Anything else is a real fault and must be seen.
      if (err.code.startsWith('STATE_') || err.code === 'CHAIN_MISMATCH') return null;
      throw err;
    }
  };

  return {
    kind: 'wasm',
    label: 'WASM ENGINE — crates/wasm (strk20-consumer compiled to wasm32)',
    provenance:
      'The real computer: the shipped consumer state machine, byte-identical to the native client on ' +
      'the same feed. Rust verifies and folds; TypeScript fetches, checks each .zst sha256 before ' +
      'inflating it with fzstd, and persists. Timings include fetch, inflate and the fold, and ' +
      'exclude wasm instantiation, reported separately as `boot`. The fetch PLAN is authored in ' +
      'TypeScript (the module has no request_log), so the request-log hash is computed there too — ' +
      'still key-blind, but asserted by the wrapper rather than proved by the module.',

    async create(profileJson: string): Promise<Engine> {
      try {
        // No engine yet: `Engine::new` needs the FETCHED genesis.json, and this
        // factory has no network. `sync_begin` fetches it as step 0 — and the
        // adapter checks it against this profile before constructing anything.
        const profile = JSON.parse(profileJson) as ChainProfile;
        const { glue, memory } = await boot();
        return guarded(new WasmEngineAdapter(glue, memory, null, null, profile));
      } catch (e) {
        throw asStrk20Error(e);
      }
    },

    async load(profileJson: string, frames: readonly Uint8Array[]): Promise<Engine | null> {
      try {
        return await loadFrames(profileJson, frames);
      } catch (e) {
        throw asStrk20Error(e);
      }
    },
  };
}
