/**
 * The stage machine and the store.
 *
 * One object, one `set`, one `subscribe`. No DOM, no engine calls, no async.
 * Everything the UI needs to decide what to enable is a pure selector at the
 * bottom of this file, so "why is Send disabled" has exactly one answer and it
 * is printable.
 */

import type { ActionKind } from './engine/chain';
import type {
  ColdStartLane,
  EngineInfo,
  FeedState,
  NetworkRecord,
  Note,
  SyncTiming,
} from './engine/types';
import { strk } from './format';

// ---------------------------------------------------------------------------
// stage
// ---------------------------------------------------------------------------

export type Stage =
  /** module not instantiated yet */
  | { s: 'boot' }
  /** opened, but no mirror for this key: nothing can be done until a sync */
  | { s: 'cold' }
  | { s: 'syncing'; mode: 'cold' | 'warm'; startedAt: number }
  | { s: 'ready' }
  /** a staged action is mid-step */
  | { s: 'acting'; kind: ActionKind; step: 0 | 1 }
  /** submitted; the log's last line is mutating until discovery sees the change */
  | { s: 'waiting'; kind: ActionKind; armedAt: number }
  | { s: 'error'; message: string };

/** The two-step "approve then swap" strip at the bottom. */
export interface ActionProgress {
  /** Which action the user has armed, if any. Arming another resets this. */
  kind: ActionKind | null;
  /** Step 1 has completed, so step 2 is unlocked. */
  stepDone: boolean;
}

export interface WaitBaseline {
  kind: ActionKind;
  armedAt: number;
  /** Captured at arm time. Resolution is "something appeared that is not here". */
  noteIds: ReadonlySet<string>;
  spentNullifiers: ReadonlySet<string>;
  /** How the resolving pass was triggered, so the number is not flattered. */
  headAtArm: number;
}

export interface ScannerState {
  hits: number;
  scanned: number;
  planted: boolean;
}

export interface AppState {
  stage: Stage;
  identityId: 'A' | 'B';
  lane: ColdStartLane;
  subscription: boolean;
  engine: EngineInfo;
  feed: FeedState;
  notes: readonly Note[];
  coldTiming: SyncTiming | null;
  warmTiming: SyncTiming | null;
  /** e.g. "browser http cache served 518 artifacts" — printed under the column. */
  coldCaveat: string | null;
  action: ActionProgress;
  waiting: WaitBaseline | null;
  network: readonly NetworkRecord[];
  /** A manual discovery pass is in flight. Feedback for it lives on the button. */
  checking: boolean;
  /** URL sequence captured per identity, for the privacy verdict. */
  captures: Partial<Record<'A' | 'B', readonly string[]>>;
  scanner: ScannerState;
  netExpanded: boolean;
}

// ---------------------------------------------------------------------------
// store
// ---------------------------------------------------------------------------

type Listener = (s: AppState) => void;

export class Store {
  private listeners = new Set<Listener>();

  constructor(private state: AppState) {}

  get(): AppState {
    return this.state;
  }

  set(patch: Partial<AppState>): void {
    this.state = { ...this.state, ...patch };
    for (const l of this.listeners) l(this.state);
  }

  subscribe(l: Listener): () => void {
    this.listeners.add(l);
    l(this.state);
    return () => this.listeners.delete(l);
  }
}

export function initialState(engine: EngineInfo, feed: FeedState): AppState {
  return {
    stage: { s: 'boot' },
    identityId: 'A',
    lane: 'epochs',
    subscription: true,
    engine,
    feed,
    notes: [],
    coldTiming: null,
    warmTiming: null,
    coldCaveat: null,
    action: { kind: null, stepDone: false },
    waiting: null,
    network: [],
    checking: false,
    captures: {},
    scanner: { hits: 0, scanned: 0, planted: false },
    netExpanded: false,
  };
}

// ---------------------------------------------------------------------------
// selectors — the single source of "why is this disabled"
// ---------------------------------------------------------------------------

export interface Gate {
  readonly enabled: boolean;
  /** Always set. Shown under the action, not hidden in a tooltip. */
  readonly reason: string;
}

export function balance(notes: readonly Note[]): number {
  return notes.filter((n) => !n.spent).reduce((a, n) => a + n.amount, 0);
}

export function unspent(notes: readonly Note[]): readonly Note[] {
  return notes.filter((n) => !n.spent);
}

const NEEDS: Readonly<Record<ActionKind, number>> = { deposit: 0, send: 25, withdraw: 10 };

/** Whether the action as a whole may be started. */
export function actionGate(s: AppState, kind: ActionKind): Gate {
  switch (s.stage.s) {
    case 'boot':
      return { enabled: false, reason: 'starting' };
    case 'cold':
      return { enabled: false, reason: 'no mirror' };
    case 'syncing':
      return { enabled: false, reason: 'syncing' };
    case 'acting':
      return s.stage.kind === kind
        ? { enabled: false, reason: 'running' }
        : { enabled: false, reason: `busy: ${s.stage.kind}` };
    case 'waiting':
      return { enabled: false, reason: 'waiting' };
    case 'error':
      return { enabled: false, reason: 'error' };
    case 'ready':
      break;
  }

  const bal = balance(s.notes);
  if (kind === 'send' || kind === 'withdraw') {
    if (unspent(s.notes).length === 0) {
      return { enabled: false, reason: 'no note yet' };
    }
    const need = NEEDS[kind];
    const biggest = Math.max(...unspent(s.notes).map((n) => n.amount));
    if (biggest < need) {
      return { enabled: false, reason: `note ${biggest.toFixed(2)} < ${need.toFixed(2)} STRK` };
    }
  }
  return { enabled: true, reason: kind === 'deposit' ? 'ready' : strk(bal) };
}

/** Whether one step button of a staged action may be pressed. */
export function stepGate(s: AppState, kind: ActionKind, step: 0 | 1): Gate {
  const base = actionGate(s, kind);
  const armed = s.action.kind === kind;

  if (step === 0) {
    if (armed && s.action.stepDone) return { enabled: false, reason: 'done' };
    return base;
  }
  if (!base.enabled) return base;
  if (!armed || !s.action.stepDone) {
    return { enabled: false, reason: 'step 1 first' };
  }
  return { enabled: true, reason: 'ready' };
}

export type StepVisual = 'todo' | 'active' | 'done' | 'locked';

export function stepVisual(s: AppState, kind: ActionKind, step: 0 | 1): StepVisual {
  const armed = s.action.kind === kind;
  if (s.stage.s === 'acting' && s.stage.kind === kind && s.stage.step === step) return 'active';
  if (armed && s.action.stepDone && step === 0) return 'done';
  if (stepGate(s, kind, step).enabled) return 'todo';
  return 'locked';
}
