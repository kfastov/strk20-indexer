/**
 * Wiring only.
 *
 * This file constructs the engine, the wallet and the views, and connects
 * events to state transitions. It contains no timings, no copy about what is
 * shipped, no DOM structure and no simulation. If you are changing how the
 * prototype LOOKS, you want index.html, styles.css or src/ui/*. If you are
 * changing what it PRETENDS, you want src/engine/*.
 */

import './styles.css';

import { SimulatedChain, type ActionKind } from './engine/chain';
import { IDENTITIES } from './engine/fixtures';
import { MockEngine } from './engine/mock';
import type {
  ColdStartLane,
  DiscoverOut,
  Identity,
  PhaseName,
  PhaseProgress,
  Strk20Engine,
  Unsubscribe,
} from './engine/types';
import * as f from './format';
import { ACTIONS, MockWallet, type Wallet } from './wallet';
import {
  actionGate,
  balance,
  initialState,
  stepGate,
  Store,
  type AppState,
  type WaitBaseline,
} from './state';
import { all, need } from './ui/dom';
import { VisibilityWatch } from './visibility';
import { LogView, type PendingHandle } from './ui/log';
import { Panels } from './ui/panels';
import { Controls } from './ui/controls';

// ===========================================================================
// THE ONE BINDING
// ---------------------------------------------------------------------------
// Swapping the mock for the real WASM-backed client is this line and nothing
// else. `Strk20Engine` (src/engine/types.ts) is the whole contract.
// ===========================================================================
const seed = Number(new URLSearchParams(location.search).get('seed') ?? Date.now() % 100_000);
const chain = new SimulatedChain(seed);
const engine: Strk20Engine = new MockEngine(chain, seed);
const wallet: Wallet = new MockWallet(chain, seed);

// ---------------------------------------------------------------------------
// views and state
// ---------------------------------------------------------------------------

const store = new Store(initialState(engine.info, engine.feed()));
const log = new LogView(need('#log-scroll'), need('#log-list'), need('#log-pending'));
const panels = new Panels();
const controls = new Controls();

store.subscribe((s) => {
  panels.render(s);
  controls.render(s);
});

const visibility = new VisibilityWatch();
let unsubscribe: Unsubscribe | null = null;
let waitLine: PendingHandle | null = null;
let passInFlight = false;
let syncInFlight = false;
let netMark = 0;

const identity = (): Identity => IDENTITIES[store.get().identityId];

// ---------------------------------------------------------------------------
// sync
// ---------------------------------------------------------------------------

/**
 * Turns the engine's phase reports into a chain of log lines whose LAST one is
 * always the mutating pending line. When the phase changes, the previous line
 * freezes with the elapsed time it actually took.
 */
function phaseDriver() {
  let handle: PendingHandle | null = null;
  let current: PhaseName | null = null;
  let detail = '';
  return {
    on(p: PhaseProgress): void {
      if (p.phase !== current) {
        handle?.resolve({ detail });
        current = p.phase;
        detail = p.detail;
        handle = log.pending(p.phase, p.detail);
      } else if (p.detail && p.detail !== detail) {
        detail = p.detail;
        handle?.update(p.detail);
      }
    },
    finish(): void {
      handle?.resolve({ detail });
      handle = null;
      current = null;
    },
  };
}

async function runSync(mode: 'cold' | 'warm'): Promise<void> {
  if (syncInFlight) return;
  syncInFlight = true;
  cancelWait();

  const id = identity();
  store.set({ stage: { s: 'syncing', mode, startedAt: performance.now() } });

  if (mode === 'cold') {
    log.append({ event: 'reset', detail: 'mirror cleared', kind: 'warn' });
  }

  netMark = engine.network().length;
  const driver = phaseDriver();
  const t0 = performance.now();

  try {
    const res = await engine.sync({ identity: id, mode, onPhase: (p) => driver.on(p) });
    driver.finish();

    const clamped = visibility.wasHiddenDuring(t0, performance.now());
    if (clamped) {
      log.append({ event: 'tab hidden', detail: 'timings clamped', kind: 'warn' });
    }

    const bal = balance(res.notes);
    log.append({
      event: mode === 'cold' ? 'cold sync' : 'warm sync',
      detail: `${res.notes.length} notes · ${f.strk(bal)} · ${res.feed.verified}`,
      kind: 'ok',
      durationMs: res.timing.totalMs,
    });

    const caveat = clamped
      ? 'tab hidden — timings clamped'
      : mode === 'cold' && engine.network().slice(netMark).some((r) => r.source === 'http-cache')
        ? 'served from http cache'
        : null;
    if (caveat && !clamped) {
      log.append({ event: 'http cache', detail: 'epochs served locally', kind: 'warn', aside: true });
    }

    const capture = engine
      .network()
      .slice(netMark)
      .filter((r) => r.synthetic === undefined)
      .map((r) => r.url);

    store.set({
      stage: { s: 'ready' },
      notes: res.notes,
      feed: res.feed,
      network: [...engine.network()],
      ...(mode === 'cold' ? { coldTiming: res.timing, coldCaveat: caveat } : { warmTiming: res.timing }),
      ...(mode === 'cold' ? { captures: { ...store.get().captures, [id.id]: capture } } : {}),
    });
  } catch (err) {
    driver.finish();
    const message = err instanceof Error ? err.message : String(err);
    log.append({ event: 'error', detail: message, kind: 'error' });
    store.set({ stage: { s: 'error', message } });
  } finally {
    syncInFlight = false;
  }
}

// ---------------------------------------------------------------------------
// discovery passes
// ---------------------------------------------------------------------------

async function runPass(trigger: 'sse' | 'manual'): Promise<void> {
  if (passInFlight || syncInFlight) return;
  const s = store.get();
  if (s.stage.s !== 'ready' && s.stage.s !== 'waiting') return;
  passInFlight = true;
  if (trigger === 'manual') store.set({ checking: true });
  try {
    const out = await engine.discover(identity());
    // A poll that found nothing is not an event. The subscription's liveness
    // lives in its toggle, its cost in the requests panel, and the moving head
    // in the feed panel — all state, all already on screen.
    if (out.added.length > 0 || out.spent.length > 0) {
      log.append({
        event: 'check',
        detail: `${trigger} · ${out.added.length} new, ${out.spent.length} spent`,
        kind: 'ok',
        durationMs: out.timing.totalMs,
      });
    }

    store.set({ notes: out.notes, feed: engine.feed(), network: [...engine.network()] });
    resolveWaitIfPossible(out, trigger);
  } finally {
    passInFlight = false;
    if (trigger === 'manual') store.set({ checking: false });
  }
}

function resolveWaitIfPossible(out: DiscoverOut, trigger: 'sse' | 'manual'): void {
  const w = store.get().waiting;
  if (!w || !waitLine) return;

  if (w.kind === 'deposit') {
    const fresh = out.notes.find((n) => !w.noteIds.has(n.id) && !n.spent);
    if (!fresh) return;
    waitLine.resolve({
      event: 'note found',
      detail: `${f.shortFelt(fresh.id)} · ${f.strk(fresh.amount)} · ${trigger}`,
      kind: 'ok',
    });
  } else {
    const gone = out.notes.find((n) => n.spent && !w.spentNullifiers.has(n.nullifier));
    if (!gone) return;
    const change = out.notes.find((n) => !w.noteIds.has(n.id) && !n.spent);
    waitLine.resolve({
      event: 'nullifier landed',
      detail:
        f.shortFelt(gone.nullifier) +
        (change ? ` · change ${f.strk(change.amount)}` : '') +
        ` · ${trigger}`,
      kind: 'ok',
    });
  }

  waitLine = null;
  store.set({ stage: { s: 'ready' }, waiting: null });
}

function cancelWait(reason = 'cancelled'): void {
  if (!waitLine) return;
  waitLine.cancel(reason);
  waitLine = null;
  if (store.get().stage.s === 'waiting') store.set({ stage: { s: 'ready' }, waiting: null });
  else store.set({ waiting: null });
}

// ---------------------------------------------------------------------------
// staged actions
// ---------------------------------------------------------------------------

async function runStep(kind: ActionKind, step: 0 | 1): Promise<void> {
  const s = store.get();
  if (!stepGate(s, kind, step).enabled) return;

  const def = ACTIONS[kind];
  const stepDef = def.steps[step];
  store.set({
    stage: { s: 'acting', kind, step },
    action: step === 0 ? { kind, stepDone: false } : s.action,
  });

  const line = log.pending(`${kind} ${step + 1}/2`, stepDef.pending);
  try {
    const out = await wallet.runStep(kind, step, identity());
    line.resolve({ event: `${kind} ${step + 1}/2`, detail: out.detail, kind: 'action' });

    if (step === 0) {
      store.set({ stage: { s: 'ready' }, action: { kind, stepDone: true } });
      return;
    }

    // Step 2 submitted. Arm a baseline and hand the last line over to the wait.
    const now = store.get();
    const baseline: WaitBaseline = {
      kind,
      armedAt: performance.now(),
      noteIds: new Set(now.notes.map((n) => n.id)),
      spentNullifiers: new Set(now.notes.filter((n) => n.spent).map((n) => n.nullifier)),
      headAtArm: now.feed.head,
    };
    store.set({
      stage: { s: 'waiting', kind, armedAt: baseline.armedAt },
      action: { kind: null, stepDone: false },
      waiting: baseline,
    });
    waitLine = log.pending(
      def.awaiting === 'note' ? 'waiting for the note' : 'waiting for the spend',
      '',
    );
  } catch (err) {
    line.fail(err instanceof Error ? err.message : String(err));
    store.set({ stage: { s: 'ready' }, action: { kind: null, stepDone: false } });
  }
}

// ---------------------------------------------------------------------------
// identity
// ---------------------------------------------------------------------------

async function switchIdentity(to: 'A' | 'B'): Promise<void> {
  if (store.get().identityId === to || syncInFlight) return;
  cancelWait();

  store.set({
    identityId: to,
    notes: [],
    action: { kind: null, stepDone: false },
    stage: { s: 'boot' },
  });

  const id = IDENTITIES[to];
  log.append({ event: 'identity', detail: id.label, kind: 'privacy' });

  const r = await engine.open(id);

  if (r.warm) {
    store.set({ stage: { s: 'ready' } });
    await runSync('warm');
  } else {
    store.set({ stage: { s: 'cold' } });
  }
}

// ---------------------------------------------------------------------------
// subscription
// ---------------------------------------------------------------------------

function setSubscription(on: boolean): void {
  store.set({ subscription: on });
  unsubscribe?.();
  unsubscribe = null;

  if (on) {
    unsubscribe = engine.subscribe((ev) => {
      if (ev.type !== 'head') return;
      store.set({ feed: engine.feed() });
      void runPass('sse');
    });
  }
}

// ---------------------------------------------------------------------------
// bindings
// ---------------------------------------------------------------------------

need<HTMLButtonElement>('#btn-run-cold').addEventListener('click', () => void runSync('cold'));
need<HTMLButtonElement>('#btn-run-warm').addEventListener('click', () => void runSync('warm'));
need<HTMLButtonElement>('#btn-check-now').addEventListener('click', () => void runPass('manual'));
need<HTMLButtonElement>('#btn-cancel-wait').addEventListener('click', () => cancelWait());

need<HTMLInputElement>('#sub-toggle').addEventListener('change', (e) => {
  setSubscription((e.currentTarget as HTMLInputElement).checked);
});

for (const btn of all<HTMLButtonElement>('.step-btn')) {
  btn.addEventListener('click', () => {
    const kind = btn.dataset['action'] as ActionKind;
    const step = Number(btn.dataset['step']) as 0 | 1;
    void runStep(kind, step);
  });
}

for (const btn of all<HTMLButtonElement>('.lane')) {
  btn.addEventListener('click', () => {
    const lane = btn.dataset['lane'] as ColdStartLane;
    if (store.get().lane === lane) return;
    engine.setLane(lane);
    store.set({ lane, feed: engine.feed() });
    log.append({
      event: 'lane',
      detail: lane === 'snapshot' ? 'snapshot' : 'epochs',
      kind: lane === 'snapshot' ? 'warn' : 'info',
    });
  });
}

for (const btn of all<HTMLButtonElement>('.ident')) {
  btn.addEventListener('click', () => void switchIdentity(btn.dataset['identity'] as 'A' | 'B'));
}

need<HTMLButtonElement>('#btn-expand-net').addEventListener('click', () => {
  store.set({ netExpanded: !store.get().netExpanded });
});

const selfTest = need<HTMLButtonElement>('#btn-scanner-selftest');
if (!engine.probe) {
  selfTest.hidden = true;
} else {
  selfTest.addEventListener('click', () => {
    const probe = engine.probe;
    if (!probe) return;
    selfTest.disabled = true;
    probe.plantKeyProbe(identity());
    store.set({ network: [...engine.network()], netExpanded: true });
    log.append({ event: 'probe', detail: 'key planted', kind: 'warn' });
    window.setTimeout(() => {
      probe.clearProbes();
      store.set({ network: [...engine.network()] });
      log.append({ event: 'probe', detail: 'cleared', kind: 'privacy' });
      selfTest.disabled = false;
    }, 6_000);
  });
}

// keyboard: the log scroller is focusable, so make it behave like one
need('#log-scroll').addEventListener('keydown', (e) => {
  const ev = e as KeyboardEvent;
  const el = need('#log-scroll');
  if (ev.key === 'End') {
    el.scrollTop = el.scrollHeight;
    ev.preventDefault();
  } else if (ev.key === 'Home') {
    el.scrollTop = 0;
    ev.preventDefault();
  }
});

// ---------------------------------------------------------------------------
// boot
// ---------------------------------------------------------------------------

async function boot(): Promise<void> {
  await engine.open(identity());
  store.set({ stage: { s: 'cold' }, network: [...engine.network()] });
  chain.start();
  setSubscription(true);
  await runSync('cold');
}

void boot();

// Expose a little for console poking during design review. Not load-bearing.
Object.assign(window as unknown as Record<string, unknown>, {
  strk20: { store, engine, chain, log, dumpState: (): AppState => store.get(), gate: actionGate },
});
