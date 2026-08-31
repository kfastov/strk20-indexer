/**
 * THE SEAM.
 *
 * `Strk20Engine` is the only thing the UI is allowed to know about. Everything
 * that will eventually be `@strk20/client` (TypeScript fetch + zstd + a
 * synchronous WASM computer folding bytes into notes) lives behind it.
 *
 * There is exactly one implementation today: `MockEngine` in `./mock.ts`.
 * To drop in the real one:
 *
 *   1. add `src/engine/wasm.ts` implementing this interface;
 *   2. change the single `createEngine()` binding in `src/main.ts`;
 *   3. change nothing else.
 *
 * The method set is deliberately shaped like the real client's, not like the
 * prototype's convenience:
 *
 *   open()      wasm instantiation + local store open; tells you cold or warm
 *   sync()      load and apply feed bytes end to end, reporting per-phase timings
 *   discover()  run discovery for one viewing key over the mirror we already have
 *   network()   every request that was issued, verbatim, for the privacy claim
 *   subscribe() feed pokes  (real: SSE /feed/live — ROADMAP ITEM 2, unbuilt)
 *   setLane()   epoch replay vs snapshot start (ROADMAP ITEM 1, unbuilt)
 *
 * Note what is NOT here: there is no `deposit`, `send` or `withdraw`. The
 * indexer has no write path and that is a deliberate project decision
 * (docs/roadmap.md, "Deferred, with triggers"). Writes go through `src/wallet.ts`,
 * which stands in for the privacy SDK and the hosted prover — someone else's
 * code. Keeping that out of this file is the point of this file.
 */

/** Lowercase 0x-prefixed minimal hex, the way the feed writes felts. */
export type Felt = string;

export type Unsubscribe = () => void;

// ---------------------------------------------------------------------------
// identity
// ---------------------------------------------------------------------------

export interface Identity {
  readonly id: 'A' | 'B';
  readonly label: string;
  readonly address: Felt;
  /** Never leaves the browser. The whole product claim is about this value. */
  readonly viewingKey: Felt;
}

// ---------------------------------------------------------------------------
// discovery output
// ---------------------------------------------------------------------------

export interface Note {
  readonly id: Felt;
  readonly token: string;
  /** Display units. The real engine returns u256 wei-equivalents. */
  readonly amount: number;
  readonly block: number;
  readonly nullifier: Felt;
  readonly spent: boolean;
}

export interface DiscoverOut {
  readonly notes: readonly Note[];
  /** Pure diff against the state blob the engine was given, not a store query. */
  readonly added: readonly Note[];
  readonly spent: readonly Note[];
  readonly timing: SyncTiming;
}

// ---------------------------------------------------------------------------
// timings
// ---------------------------------------------------------------------------

export type PhaseName =
  | 'boot'
  | 'open'
  | 'plan'
  | 'fetch'
  | 'inflate'
  | 'verify+fold'
  | 'snapshot'
  | 'load'
  | 'apply'
  | 'discover'
  | 'export';

export interface Phase {
  readonly name: PhaseName;
  readonly ms: number;
  readonly detail?: string;
  /**
   * Set on a warm run for the phases a warm run does not perform. The UI strikes
   * these through rather than printing a small number, because "absent" and
   * "fast" are different claims.
   */
  readonly skipped?: boolean;
  readonly skippedBytes?: number;
}

export interface SyncTiming {
  readonly totalMs: number;
  readonly phases: readonly Phase[];
}

// ---------------------------------------------------------------------------
// feed state
// ---------------------------------------------------------------------------

/** §1.5.1 of docs/spec/consumer-path.md — the grade is surfaced, never implied. */
export type VerifiedGrade = 'replayed' | 'anchored' | 'server-asserted';

export interface FeedState {
  readonly chainId: string;
  readonly pool: Felt;
  readonly head: number;
  readonly l1Accepted: number;
  readonly latestEpoch: number;
  readonly epochCount: number;
  readonly feedBytes: number;
  readonly verified: VerifiedGrade;
  /** manifest.snapshot !== null. False in every shipped feed today. */
  readonly snapshotAvailable: boolean;
}

// ---------------------------------------------------------------------------
// network records — the privacy evidence
// ---------------------------------------------------------------------------

export interface NetworkRecord {
  readonly method: 'GET';
  /** Exactly what went in the request line. No query strings, ever. */
  readonly url: string;
  readonly status: number;
  readonly bytes: number;
  readonly ms: number;
  readonly source: 'network' | 'http-cache';
  /** Present only on rows the prototype injects to prove a point. */
  readonly synthetic?: string;
}

// ---------------------------------------------------------------------------
// cold-start lanes
// ---------------------------------------------------------------------------

/**
 * `epochs` — replay every epoch from pool genesis. This is what ships today.
 * `snapshot` — start from a folded slot snapshot at an epoch boundary.
 *   ROADMAP ITEM 1, unbuilt: `manifest.snapshot` is `null` in the real feed.
 */
export type ColdStartLane = 'epochs' | 'snapshot';

// ---------------------------------------------------------------------------
// events the engine pushes at the host
// ---------------------------------------------------------------------------

export type EngineEvent =
  | { readonly type: 'head'; readonly head: number; readonly l1Accepted: number }
  | { readonly type: 'epoch'; readonly e: number; readonly from: number; readonly to: number }
  | { readonly type: 'status'; readonly decodeState: 'ok' | 'degraded' };

export interface PhaseProgress {
  readonly phase: PhaseName;
  readonly detail: string;
  /** 0..1 where the engine can say; undefined where it genuinely cannot. */
  readonly fraction?: number;
}

// ---------------------------------------------------------------------------
// requests
// ---------------------------------------------------------------------------

export interface OpenResult {
  readonly warm: boolean;
  readonly store: 'indexeddb' | 'memory';
  readonly persisted: boolean;
  readonly timing: SyncTiming;
}

export interface SyncRequest {
  readonly identity: Identity;
  /** `cold` clears the local store first, exactly like the real `resetCache()`. */
  readonly mode: 'cold' | 'warm';
  readonly signal?: AbortSignal;
  readonly onPhase?: (p: PhaseProgress) => void;
}

export interface SyncResult extends DiscoverOut {
  readonly feed: FeedState;
  readonly mode: 'cold' | 'warm';
  readonly lane: ColdStartLane;
}

// ---------------------------------------------------------------------------
// engine identity — shown in the UI so a screenshot cannot lie
// ---------------------------------------------------------------------------

export interface EngineInfo {
  /** Rendered verbatim in the banner. */
  readonly name: string;
  readonly build: string;
  /** True for anything that does not run the real discovery-core over real bytes. */
  readonly simulated: boolean;
  /** One sentence the UI prints next to the name. */
  readonly notice: string;
}

// ---------------------------------------------------------------------------
// the interface
// ---------------------------------------------------------------------------

/**
 * Optional, prototype-only. Lets the UI demonstrate that the key scanner is not
 * vacuous by planting a request that DOES carry the key (the compat wire, which
 * receives viewing keys by protocol definition and is off by default).
 *
 * A real engine may leave this undefined; the UI hides the button when it is.
 * It is optional precisely so the seam does not force the real client to grow a
 * method that can emit a key-bearing URL.
 */
export interface EngineProbe {
  plantKeyProbe(identity: Identity): void;
  clearProbes(): void;
}

export interface Strk20Engine {
  readonly info: EngineInfo;
  readonly probe?: EngineProbe;

  /** Instantiate the module and open the local store for this key. */
  open(identity: Identity): Promise<OpenResult>;

  /** Load and apply feed bytes, then discover. The whole cold/warm path. */
  sync(req: SyncRequest): Promise<SyncResult>;

  /** One discovery pass over the mirror we already hold. The "check now" path. */
  discover(identity: Identity, signal?: AbortSignal): Promise<DiscoverOut>;

  /** Every request issued since `open`, oldest first. */
  network(): readonly NetworkRecord[];

  /** Drop the local mirror and the sealed state blob for this key. */
  clearLocalState(identity: Identity): Promise<void>;

  /** Feed pokes. Real implementation: EventSource on /feed/live. */
  subscribe(handler: (ev: EngineEvent) => void): Unsubscribe;

  setLane(lane: ColdStartLane): void;
  lane(): ColdStartLane;

  /** Current feed view without syncing (manifest read). */
  feed(): FeedState;

  close(): Promise<void>;
}

/**
 * The one binding the real client replaces. `main.ts` calls this and nothing
 * else constructs an engine.
 */
export type EngineFactory = () => Strk20Engine;
