/**
 * demo-app.md §5 — the state machine, the log and the pending line.
 *
 * The rules below are enforced HERE, in the reducer, rather than by discipline
 * in the rendering code. Leg d5 asserts them over a recorded trace:
 *   - at most one `pending` line exists, and it is always the last;
 *   - a pending line mutates in place and commits with its elapsed time;
 *   - committed lines never mutate;
 *   - a committed line may legally carry NO elapsed time (§5.4's deadline).
 */

import type { FeedState, Note, RequestRecord } from 'strk20-discovery';

export type Lane = 'replay' | 'mainnet-local' | 'live';
/**
 * `unavailable` exists because the real engine cannot report every counter the
 * mock could. A slot with no measurement behind it says so; it never renders a
 * 0 under a badge that claims the number was measured.
 */
export type Provenance = 'measured' | 'recorded' | 'derived' | 'unavailable';
export type Trigger = 'manual' | 'sse' | 'poll';
export type ActionKind = 'deposit' | 'send' | 'withdraw';

export interface Metric {
  label: string;
  value: string;
  provenance: Provenance;
}

export interface LogLine {
  seq: number;
  at: number;
  lane: Lane;
  stage: 'feed' | 'identity' | 'discover' | 'await' | 'network' | 'error';
  text: string;
  pendingText?: string;
  status: 'pending' | 'ok' | 'warn' | 'fail';
  /** Frozen at commit. ABSENT is legal and meaningful (§5.4). */
  elapsedMs?: number;
  metrics?: Metric[];
  detail?: string;
}

/** A measured column. `null` means "not run yet" and renders as such — never 0. */
export interface RunCard {
  kind: 'cold' | 'warm' | 'warm-reload';
  ranAt: string | null;
  lane: Lane | null;
  feedUrl: string | null;
  totalMs: number | null;
  fetchMs: number | null;
  inflateMs: number | null;
  applyMs: number | null;
  loadMs: number | null;
  discoverMs: number | null;
  networkRequests: number | null;
  cacheRequests: number | null;
  bytes: number | null;
  /** Epoch artifacts this run REQUESTED. Zero on a warm start. */
  epochs: number | null;
  /** Epochs the engine FOLDED. 607 on both a cold and a warm start here. */
  epochsApplied: number | null;
  bootMs: number | null;
  /** Set when the column could not be measured, with the reason. */
  unavailable: string | null;
  /** Bytes that did NOT have to be downloaded, for the struck-through rows. */
  bytesSaved: number | null;
}

export function emptyCard(kind: RunCard['kind']): RunCard {
  return {
    kind,
    ranAt: null,
    lane: null,
    feedUrl: null,
    totalMs: null,
    fetchMs: null,
    inflateMs: null,
    applyMs: null,
    loadMs: null,
    discoverMs: null,
    networkRequests: null,
    cacheRequests: null,
    bytes: null,
    epochs: null,
    epochsApplied: null,
    bootMs: null,
    unavailable: null,
    bytesSaved: null,
  };
}

export interface IdentityState {
  id: 'A' | 'B' | 'generated';
  address: `0x${string}`;
  keyIdPrefix: string;
}

export interface AbResult {
  status: 'identical' | 'different' | 'incomparable';
  hashA: string;
  hashB: string;
  bytesA: number;
  bytesB: number;
  requestsA: number;
  requestsB: number;
  manifestA: string;
  manifestB: string;
}

export interface OpState {
  kind: ActionKind;
  armedAt: number;
  baselineNoteIds: Set<string>;
  baselineSpent: Set<string>;
  target: { kind: 'note' } | { kind: 'nullifier'; noteId: string };
  pokes: number;
  lineSeq: number;
}

export interface DemoState {
  lane: Lane;
  replayStage: 't0' | 't1' | 't2';
  feedUrl: string;
  /** `provenance` is never rendered as text; it is the badge's title attribute. */
  engine: { kind: 'wasm' | 'mock'; provenance: string };
  log: LogLine[];
  cold: RunCard;
  warm: RunCard;
  identity: IdentityState | null;
  notes: Note[];
  balances: Map<string, bigint>;
  feed: FeedState | null;
  records: RequestRecord[];
  ab: AbResult | null;
  op: OpState | null;
  transport: 'live' | 'polling' | 'degraded' | 'idle';
  subscription: boolean;
  persistence: string;
  persisted: boolean;
  scanHits: number;
  scanSurfaces: number;
  selfTestFired: boolean | null;
  busy: string | null;
  /** Drives the `check now` control's own `checking…` state — never a log line. */
  checking: boolean;
  stage1Done: boolean;
  discoveryRan: boolean;
}

export function initialState(lane: Lane, feedUrl: string, engine: DemoState['engine']): DemoState {
  return {
    lane,
    replayStage: 't0',
    feedUrl,
    engine,
    log: [],
    cold: emptyCard('cold'),
    warm: emptyCard('warm'),
    identity: null,
    notes: [],
    balances: new Map(),
    feed: null,
    records: [],
    ab: null,
    op: null,
    transport: 'idle',
    subscription: false,
    persistence: 'unknown',
    persisted: false,
    scanHits: 0,
    scanSurfaces: 0,
    selfTestFired: null,
    busy: null,
    checking: false,
    stage1Done: false,
    discoveryRan: false,
  };
}

// ------------------------------------------------------------------ the log

let seqCounter = 0;

export function pendingLine(
  s: DemoState,
  stage: LogLine['stage'],
  pendingText: string,
): LogLine {
  if (s.log.some((l) => l.status === 'pending')) {
    // Not an error the user can cause: the stage gating refuses a new operation
    // while one is pending. Reaching here is a bug, and it should be loud.
    throw new Error('invariant: a pending log line already exists');
  }
  const line: LogLine = {
    seq: ++seqCounter,
    at: performance.now(),
    lane: s.lane,
    stage,
    text: pendingText,
    pendingText,
    status: 'pending',
  };
  s.log.push(line);
  return line;
}

export interface CommitOpts {
  text: string;
  status: 'ok' | 'warn' | 'fail';
  /** Omit deliberately for §5.4's deadline: no elapsed time, no latency claim. */
  elapsedMs?: number;
  metrics?: Metric[];
  detail?: string;
}

export function commit(s: DemoState, line: LogLine, o: CommitOpts): void {
  if (line.status !== 'pending') throw new Error('invariant: committed lines never mutate');
  const last = s.log[s.log.length - 1];
  if (last !== line) throw new Error('invariant: the pending line must be last');
  line.text = o.text;
  line.status = o.status;
  delete line.pendingText;
  if (o.elapsedMs !== undefined) line.elapsedMs = o.elapsedMs;
  if (o.metrics) line.metrics = o.metrics;
  if (o.detail) line.detail = o.detail;
}

/** A line with no pending phase — logged and committed in one move. */
export function record(s: DemoState, stage: LogLine['stage'], o: CommitOpts): LogLine {
  const line: LogLine = {
    seq: ++seqCounter,
    at: performance.now(),
    lane: s.lane,
    stage,
    text: o.text,
    status: o.status,
  };
  if (o.elapsedMs !== undefined) line.elapsedMs = o.elapsedMs;
  if (o.metrics) line.metrics = o.metrics;
  if (o.detail) line.detail = o.detail;
  // A pending line must stay last, so a plain record slots in before it.
  const lastIdx = s.log.length - 1;
  if (lastIdx >= 0 && s.log[lastIdx]!.status === 'pending') s.log.splice(lastIdx, 0, line);
  else s.log.push(line);
  return line;
}

/** For leg d5: the invariants as a predicate, checkable over any trace. */
export function logInvariantsHold(log: readonly LogLine[]): true | string {
  const pending = log.filter((l) => l.status === 'pending');
  if (pending.length > 1) return `${pending.length} pending lines exist; at most one is allowed`;
  if (pending.length === 1 && log[log.length - 1] !== pending[0]) {
    return 'the pending line is not the last line';
  }
  return true;
}

// ------------------------------------------------------ the waiting baseline

export function armOp(
  s: DemoState,
  kind: ActionKind,
  target: OpState['target'],
  lineSeq: number,
): OpState {
  const op: OpState = {
    kind,
    armedAt: performance.now(),
    // Comparing against a CAPTURED baseline rather than "the count went up" is
    // what makes the elapsed number mean anything.
    baselineNoteIds: new Set(s.notes.map((n) => n.noteId)),
    baselineSpent: new Set(s.notes.filter((n) => n.spent).map((n) => n.noteId)),
    target,
    pokes: 0,
    lineSeq,
  };
  s.op = op;
  return op;
}

/**
 * Resolution is a DIFF against the armed baseline, never a timer and never a
 * poke count (§9.1). Returns the note that resolved it, or null.
 */
export function resolveOp(op: OpState, notes: readonly Note[]): Note | null {
  if (op.target.kind === 'note') {
    return notes.find((n) => !op.baselineNoteIds.has(n.noteId)) ?? null;
  }
  return notes.find((n) => n.spent && !op.baselineSpent.has(n.noteId)) ?? null;
}

// ------------------------------------------------------------ the chain clock

/**
 * The chain-side clock of a resolved operation: wall clock now, minus the block
 * timestamp of the note that resolved it. It is the second of the two latency
 * numbers a resolved line carries, and the only one this page does not control;
 * `elapsedMs` is the other.
 *
 * Neither engine the demo runs supplies a usable timestamp today, and neither
 * failure is visible in the subtraction:
 *
 *   - `engine-wasm.ts` sets `blockTimestamp: 0` on every note, because the
 *     second-pass ABI does not carry one. Subtracting it prints the whole Unix
 *     epoch in seconds under a `measured` badge.
 *   - the REPLAY fixture derives `ts` from the block number at generation time
 *     (`scripts/gen-replay-feed.mjs`: `1756000000 + (b - genesis) * 3`), so the
 *     subtraction measures the FIXTURE'S AGE rather than any latency.
 *
 * `chainClock` is therefore the caller's statement that this lane's timestamps
 * came from a chain and not from a generator; the rest is a plausibility check.
 * A slot failing any of them says `unavailable` and prints no number, which is
 * the rule `counted()` in main.ts already follows.
 */
export function endToEndMetric(
  blockTimestampSec: number,
  nowMs: number,
  chainClock: boolean,
): Metric {
  const deltaMs = nowMs - blockTimestampSec * 1000;
  if (!chainClock || blockTimestampSec <= 0 || deltaMs < 0) {
    return { label: 'end-to-end', value: 'unavailable', provenance: 'unavailable' };
  }
  return { label: 'end-to-end', value: `${Math.round(deltaMs / 1000)} s`, provenance: 'measured' };
}
