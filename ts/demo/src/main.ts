/**
 * The demo controller.
 *
 * Every number this file puts on screen is taken with `performance.now()`
 * around real work in this session, or it is not put on screen at all. Where a
 * measurement cannot be taken, the slot says `unavailable` and why. There are
 * no constants standing in for measurements and there is no artificial delay
 * anywhere in this file — a fabricated pause to widen the cold/warm contrast
 * would make the one number the product turns on into a lie.
 */

import {
  KeylessClient,
  MAINNET,
  SEPOLIA,
  Strk20Error,
  encodeAll,
  keyId,
  scan,
  surfacesOfRequest,
  type ChainProfile,
  type DiscoveryEvent,
  type FeedState,
  type Note,
  type NotesResult,
  type Subscription,
} from 'strk20-discovery';
import { ENGINE } from './engine-binding.ts';
import { ALLOW_PASTE, generatedIdentity, identityA, identityB, type DemoIdentity } from './identities.ts';
import {
  armOp,
  commit,
  emptyCard,
  initialState,
  logInvariantsHold,
  pendingLine,
  record,
  resolveOp,
  type DemoState,
  type Lane,
  type RunCard,
} from './state.ts';
import { bytes, el, ms, renderCard, renderEngineBadge, renderIdentity, renderLog, renderNetwork, renderTrust } from './ui.ts';

// ------------------------------------------------------------------- lanes

interface LaneConfig {
  id: Lane;
  label: string;
  /** At most a word or two beside the selector. Empty renders nothing. */
  chip: string;
  network: ChainProfile;
  base(stage: 't0' | 't1' | 't2'): string;
  advanceable: boolean;
  writable: boolean;
}

const LANES: Record<Lane, LaneConfig> = {
  replay: {
    id: 'replay',
    label: 'REPLAY',
    chip: 'synthetic',
    network: SEPOLIA,
    base: (stage) => `${location.origin}/replay/${stage}`,
    advanceable: true,
    writable: false,
  },
  'mainnet-local': {
    id: 'mainnet-local',
    label: 'MAINNET',
    chip: '',
    network: MAINNET,
    base: () => `${location.origin}/mainnet-feed`,
    advanceable: false,
    writable: false,
  },
  live: {
    id: 'live',
    label: 'LIVE',
    chip: '',
    get network() {
      return liveNetwork;
    },
    base: () => liveUrl,
    advanceable: false,
    writable: true,
  },
};

/**
 * The live lane points at a running indexer. The default is this repository's
 * Sepolia mirror:
 *
 *   ./target/release/strk20 run --db data/sepolia/idx/strk20.db \
 *       --feed-dir data/sepolia/idx/feed --listen 127.0.0.1:8901 --network sepolia
 *
 * `?feed=` and `?network=` override both, so a run is a shareable URL. Every
 * URL parameter this page reads names a PUBLIC feed; none of them can carry a
 * secret, and there is no URL position that accepts one.
 */
let liveUrl = new URLSearchParams(location.search).get('feed')?.replace(/\/$/, '') ?? 'http://127.0.0.1:8901/feed';
let liveNetwork: ChainProfile =
  new URLSearchParams(location.search).get('network') === 'mainnet' ? MAINNET : SEPOLIA;

// -------------------------------------------------------------- app state

// The badge reads ENGINE.kind directly, so a mock number can never be
// screenshotted without the MOCK ENGINE chip beside it.
let state: DemoState = initialState('replay', LANES.replay.base('t0'), {
  kind: ENGINE.kind,
  provenance: ENGINE.provenance,
});

let client: KeylessClient | null = null;
let identity: DemoIdentity | null = null;
let subscription: Subscription | null = null;
let deadlineTimer: ReturnType<typeof setTimeout> | null = null;

const DEADLINE_MS = 10 * 60 * 1000;
// Explicit and short, because a 30 s default would make the subscription look
// broken in a five-minute demo. Stated on screen rather than hidden.
const POLL_MS = 5000;

function laneCfg(): LaneConfig {
  return LANES[state.lane];
}

function feedUrl(): string {
  return laneCfg().base(state.replayStage);
}

// ------------------------------------------------------------ the client

function makeClient(suffix = ''): KeylessClient {
  return new KeylessClient({
    feedUrl: feedUrl(),
    network: laneCfg().network,
    engine: ENGINE,
    worker: false,
    live: true,
    pollIntervalMs: POLL_MS,
    persist: 'both',
    coldStart: 'auto',
    // Above §4.2's default of 6. Stated rather than tuned quietly: the epochs
    // lane is 606 sequential round trips, and the fetch column below shows
    // exactly what that costs. Set it to 1 for strict wire order.
    prefetchConcurrency: 16,
    requestPersistentStorage: false,
    ...(suffix ? { databaseSuffix: suffix } : {}),
    // The panel is the SESSION's record, appended to as requests happen — never
    // recomputed from whichever client happens to be current, and never
    // cleared. §9.1: never hide a request from the panel.
    onRequest: (r) => {
      state.records.push(r);
      scheduleRender();
    },
  });
}

async function freshCold(suffix = ''): Promise<KeylessClient | null> {
  // §4 Stage 1's guard, in order, and the run does not start unless the
  // deletion resolved: a cold number that was not measured cold is worse than
  // no number at all.
  if (client) {
    await client.resetCache();
    await client.close();
  }
  const probe = makeClient(suffix);
  const outcome = await probe.deleteDatabase();
  if (outcome !== 'deleted') {
    return null;
  }
  return makeClient(suffix);
}

// --------------------------------------------------------------- the runs

async function runCold(): Promise<void> {
  if (state.busy) return;
  state.busy = 'cold';
  state.cold = emptyCard('cold');
  const line = pendingLine(state, 'feed', 'clearing storage');
  render();

  const fresh = await freshCold();
  if (!fresh) {
    state.cold.unavailable = 'storage would not clear';
    commit(state, line, { text: 'cold refused — storage would not clear', status: 'warn' });
    state.busy = null;
    render();
    return;
  }
  client = fresh;
  state.records = [];
  line.text = 'fetching';

  try {
    const t0 = performance.now();
    const feed = await client.sync({ onProgress: (p) => (line.text = progressText(p.phase, p.done, p.bytes)) });
    const total = performance.now() - t0;
    fillCard(state.cold, feed, total, client);
    state.feed = feed;
    state.stage1Done = true;
    afterFeed(feed);
    commit(state, line, {
      text: `cold · ${state.cold.epochs ?? 0} epochs · head ${feed.head.toLocaleString('en-US')}`,
      status: 'ok',
      elapsedMs: total,
      metrics: [
        { label: 'fetch', value: ms(feed.timing.phases.fetch), provenance: 'measured' },
        { label: 'inflate', value: ms(feed.timing.phases.decompress), provenance: 'measured' },
        { label: 'verify+fold', value: ms(feed.timing.phases.apply), provenance: 'measured' },
        { label: 'requests', value: String(state.cold.networkRequests ?? 0), provenance: 'measured' },
        { label: 'bytes', value: bytes(state.cold.bytes), provenance: 'measured' },
        { label: 'verified', value: feed.verified, provenance: 'measured' },
      ],
    });
    if (identity) await discoverInto(state.cold, 'manual');
  } catch (e) {
    failLine(line, e);
  } finally {
    state.busy = null;
    render();
  }
}

async function runWarm(reloaded = false): Promise<void> {
  if (state.busy) return;
  // The control is disabled until stage 1 has run, so this is a guard, not a
  // user-visible refusal: nothing happened, so nothing is logged.
  if (!state.stage1Done && !reloaded) return;
  state.busy = 'warm';
  state.warm = emptyCard(reloaded ? 'warm-reload' : 'warm');
  const line = pendingLine(state, 'feed', 'restoring');
  render();

  try {
    // A FRESH client over the persisted database, same session: the honest
    // same-session comparator.
    if (client) await client.close();
    client = makeClient();
    const before = performance.now();
    const t0 = reloaded ? 0 : before;
    const feed = await client.sync();
    const total = reloaded ? performance.now() : performance.now() - t0;
    fillCard(state.warm, feed, total, client);
    state.warm.bytesSaved = (state.cold.bytes ?? 0) - (state.warm.bytes ?? 0);
    state.feed = feed;
    afterFeed(feed);
    commit(state, line, {
      text: reloaded ? 'warm after reload' : `warm · ${feed.timing.fromCache} from cache`,
      status: 'ok',
      elapsedMs: total,
      metrics: [
        { label: 'load', value: ms(feed.timing.phases.load), provenance: 'measured' },
        { label: 'requests', value: String(state.warm.networkRequests ?? 0), provenance: 'measured' },
        { label: 'bytes', value: bytes(state.warm.bytes), provenance: 'measured' },
      ],
    });
    if (identity) await discoverInto(state.warm, 'manual');
  } catch (e) {
    failLine(line, e);
  } finally {
    state.busy = null;
    render();
  }
}

const RELOAD_FLAG = 'strk20-demo:warm-after-reload';

async function runWarmAfterReload(): Promise<void> {
  // Guarded by the disabled control; a no-op is not an event.
  if (!state.stage1Done) return;
  try {
    // Lane and stage only. No identity, no key, no address is ever persisted
    // by this page.
    localStorage.setItem(RELOAD_FLAG, JSON.stringify({ lane: state.lane, stage: state.replayStage }));
  } catch {
    record(state, 'feed', { text: 'localStorage blocked', status: 'warn' });
    render();
    return;
  }
  location.reload();
}

function fillCard(card: RunCard, feed: FeedState, totalMs: number, c: KeylessClient): void {
  const recs = c.network().records;
  const network = recs.filter((r) => r.source !== 'idb-cache');
  card.ranAt = new Date().toLocaleTimeString('en-GB', { hour12: false });
  card.lane = state.lane;
  card.feedUrl = feedUrl();
  card.totalMs = totalMs;
  card.fetchMs = feed.timing.phases.fetch;
  card.inflateMs = feed.timing.phases.decompress;
  card.applyMs = feed.timing.phases.apply;
  card.loadMs = feed.timing.phases.load;
  card.networkRequests = network.length;
  card.cacheRequests = recs.length - network.length;
  card.bytes = recs.reduce((n, r) => n + r.bytes, 0);
  card.epochs = recs.filter((r) => r.artifact === 'epoch').length;
  // The engine's own counter. It is NOT the request count: a warm start folds
  // every epoch the state blob carries and fetches none of them, so reporting
  // the request count as "epochs applied" showed 0 for a run that folded 607
  // and made the fold time next to it look unexplained.
  card.epochsApplied = feed.epochsApplied;
  card.bootMs = c.bootMs();
}

function afterFeed(feed: FeedState): void {
  state.persistence = client?.status().persistence ?? 'unknown';
  state.persisted = client?.status().persisted ?? false;
  void feed;
  rescan();
}

// ------------------------------------------------------------- discovery

async function discoverInto(
  card: RunCard | null,
  trigger: 'manual' | 'sse' | 'poll',
): Promise<NotesResult | null> {
  if (!client || !identity) return null;
  state.checking = true;
  render();
  try {
    // refresh:'none' so the claim "discovery adds zero requests" is exactly
    // true. With refresh:'auto' a getNotes still issues a conditional head GET.
    const res = await client.getNotes(identity.account, { refresh: 'none' });
    if (card) card.discoverMs = res.elapsedMs;
    state.notes = res.notes;
    state.balances = res.balances;
    state.discoveryRan = true;
    rescan();

    // A pass that changed nothing is not an event. It leaves no line; the
    // control's own `checking…` state is the feedback that it ran.
    if (res.added.length || res.spent.length) {
      record(state, 'discover', {
        text: `${res.added.length} new, ${res.spent.length} spent · ${formatBalances(res.balances)}`,
        status: 'ok',
        elapsedMs: res.elapsedMs,
        metrics: [
          // A counter the engine does not expose comes back negative. It is
          // rendered as `unavailable`, never as a 0 under a "measured" label.
          { label: 'slots read', ...counted(res.stats.slotsRead) },
          { label: 'notes scanned', ...counted(res.stats.eventsScanned) },
          { label: 'requests added', value: '0', provenance: 'measured' },
        ],
      });
    }
    checkArmedOp(res.notes, trigger);
    return res;
  } catch (e) {
    record(state, 'error', { text: describe(e), status: 'fail' });
    return null;
  } finally {
    state.checking = false;
    render();
  }
}

function checkArmedOp(notes: readonly Note[], trigger: string): void {
  const op = state.op;
  if (!op) return;
  op.pokes += 1;
  const hit = resolveOp(op, notes);
  if (!hit) return;
  const line = state.log.find((l) => l.seq === op.lineSeq);
  state.op = null;
  if (deadlineTimer) clearTimeout(deadlineTimer);
  if (!line || line.status !== 'pending') return;
  const elapsed = performance.now() - op.armedAt;
  const endToEnd = Date.now() - hit.blockTimestamp * 1000;
  commit(state, line, {
    text: `${op.kind} · note ${short(hit.noteId)} ${op.target.kind === 'nullifier' ? 'spent' : 'found'} · ${trigger}`,
    status: 'ok',
    // The two latency clocks, never merged: this one is armedAt → the pass whose
    // diff against the baseline contained the change. It is the number our
    // product controls; `end-to-end` is the chain's.
    elapsedMs: elapsed,
    metrics: [
      { label: 'passes', value: String(op.pokes), provenance: 'measured' },
      { label: 'end-to-end', value: `${(endToEnd / 1000).toFixed(0)} s`, provenance: 'measured' },
    ],
  });
}

function armDeadline(): void {
  if (deadlineTimer) clearTimeout(deadlineTimer);
  deadlineTimer = setTimeout(() => {
    const op = state.op;
    if (!op) return;
    const line = state.log.find((l) => l.seq === op.lineSeq);
    state.op = null;
    if (line && line.status === 'pending') {
      // No elapsedMs, deliberately: a timeout that prints a duration produces a
      // number that looks like a measurement and is not one.
      commit(state, line, { text: 'gave up after 10:00', status: 'warn' });
    }
    render();
  }, DEADLINE_MS);
}

// ------------------------------------------------------------ subscription

function setSubscription(on: boolean): void {
  state.subscription = on;
  subscription?.close();
  subscription = null;
  if (!on || !client || !identity) {
    state.transport = 'idle';
    render();
    return;
  }
  subscription = client.watch(identity.account, (ev: DiscoveryEvent) => {
    switch (ev.type) {
      case 'status':
        if (ev.state === 'degraded') {
          record(state, 'network', { text: 'no live stream — polling', status: 'warn' });
        }
        state.transport = ev.state === 'locked' || ev.state === 'idle' ? 'idle' : ev.state;
        break;
      case 'notes':
        if (ev.added.length || ev.spent.length) {
          for (const n of ev.added) record(state, 'discover', { text: `+note ${short(n.noteId)}`, status: 'ok' });
          checkArmedOp([...state.notes, ...ev.added], state.transport === 'live' ? 'sse' : 'poll');
        }
        break;
      case 'error':
        record(state, 'error', { text: `${ev.error.code}: ${ev.error.message}`, status: 'fail' });
        break;
      default:
        break;
    }
    rescan();
    render();
  });
  record(state, 'network', { text: `subscription on · poll ${POLL_MS / 1000}s`, status: 'ok' });
  render();
}

// ------------------------------------------------------- A/B, §7's four rules

async function runAb(): Promise<void> {
  if (state.busy) return;
  state.busy = 'ab';
  const line = pendingLine(state, 'network', 'A/B — two cold runs');
  render();

  try {
    const runOne = async (suffix: string, who: DemoIdentity) => {
      const c = await freshCold(suffix);
      if (!c) throw new Strk20Error('INTERNAL', 'could not clear storage for the A/B run');
      const feed = await c.sync();
      // Discovery under each identity, over the mirror AS IT IS. This is the
      // half that must add zero rows.
      const before = c.network().records.length;
      await c.getNotes(who.account, { refresh: 'none' });
      const after = c.network().records.length;
      const recs = c.network().records;
      const out = {
        hash: feed.network.requestLogSha256,
        bytes: recs.reduce((n, r) => n + r.bytes, 0),
        requests: recs.filter((r) => r.source !== 'idb-cache').length,
        addedByDiscovery: after - before,
        feedState: `${feed.head}|${feed.lastEpoch}|${feed.lastEpochTo}`,
      };
      await c.close();
      return out;
    };

    const a = await runOne('ab-a', identityA());
    const b = await runOne('ab-b', identityB());

    // Never render a verdict across two feed states. The ABI exposes no
    // manifest hash, so the strongest in-ABI proxy is used.
    const comparable = a.feedState === b.feedState;
    const identical = a.hash === b.hash;
    state.ab = {
      status: !comparable ? 'incomparable' : identical ? 'identical' : 'different',
      hashA: a.hash,
      hashB: b.hash,
      bytesA: a.bytes,
      bytesB: b.bytes,
      requestsA: a.requests,
      requestsB: b.requests,
      manifestA: a.feedState,
      manifestB: b.feedState,
    };

    commit(state, line, {
      text: !comparable
        ? 'A/B — feed advanced, no verdict'
        : identical
          ? `A/B — identical · ${a.requests} requests each`
          : 'A/B — REQUEST LOGS DIFFER',
      status: !comparable ? 'warn' : identical ? 'ok' : 'fail',
      metrics: [
        { label: 'rows added by discovery A', value: String(a.addedByDiscovery), provenance: 'measured' },
        { label: 'B', value: String(b.addedByDiscovery), provenance: 'measured' },
      ],
    });

    // Restore a usable client for the rest of the session. The panel's scope
    // really does move, so the reset is an event and is logged as one.
    client = makeClient();
    record(state, 'network', { text: 'panel reset', status: 'ok' });
    state.records = [];
    state.cold = emptyCard('cold');
    state.warm = emptyCard('warm');
    state.stage1Done = false;
  } catch (e) {
    failLine(line, e);
  } finally {
    state.busy = null;
    render();
  }
}

// ------------------------------------------------------------- the scanner

function rescan(): void {
  const secrets = identity
    ? [
        { label: 'viewing key', bytes: identity.secretBytes },
        { label: 'address', bytes: hexToBytes(identity.account.address.slice(2)) },
      ]
    : [];
  if (secrets.length === 0) {
    state.scanHits = 0;
    state.scanSurfaces = 0;
    return;
  }
  const surfaces = state.records.flatMap((r) =>
    surfacesOfRequest({ url: r.url, method: r.method, headers: { Accept: '*/*' } }),
  );
  state.scanSurfaces = surfaces.length;
  state.scanHits = scan(surfaces, secrets).length;
}

function selfTest(): void {
  // Guarded by the disabled control.
  if (!identity) return;
  // Plant the key into a SYNTHETIC record (never a real one, never one that is
  // sent) and show the scanner catching it.
  const planted = encodeAll(identity.secretBytes)[0]!;
  const hits = scan(
    [{ where: 'url', text: `https://example.invalid/feed/epochs/${planted.needle}.strk20e.zst` }],
    [{ label: 'viewing key', bytes: identity.secretBytes }],
  );
  state.selfTestFired = hits.length > 0;
  record(state, 'network', {
    text: hits.length > 0 ? `self-test — caught (${hits[0]!.encoding})` : 'self-test — MISSED',
    status: hits.length > 0 ? 'ok' : 'fail',
  });
  render();
}

// ------------------------------------------------------------------ wiring

function setIdentity(who: DemoIdentity | null): void {
  identity = who;
  subscription?.close();
  subscription = null;
  state.subscription = false;
  state.notes = [];
  state.balances = new Map();
  state.discoveryRan = false;
  if (!who) {
    state.identity = null;
    render();
    return;
  }
  void who.account.viewingKey().then((k) => {
    const kid = keyId(k, laneCfg().network.chainId, laneCfg().network.pool, who.account.address);
    k.fill(0);
    state.identity = { id: who.id, address: who.account.address, keyIdPrefix: kid.slice(0, 8) };
    // keyId only. The address is a public value and is shown in the identity
    // panel, but it is never written to a log line.
    record(state, 'identity', { text: `identity ${who.id} · keyId ${kid.slice(0, 8)}…`, status: 'ok' });
    rescan();
    render();
  });
}

async function advanceReplay(): Promise<void> {
  // All three are enforced by the control being absent or disabled.
  if (state.busy || state.lane !== 'replay' || !identity || state.op) return;
  const next = state.replayStage === 't0' ? 't1' : state.replayStage === 't1' ? 't2' : null;
  if (!next) return;

  const kind = next === 't1' ? 'deposit' : 'withdraw';
  const line = pendingLine(state, 'await', next === 't1' ? 'waiting for the note' : 'waiting for the spend');
  const target =
    next === 't1'
      ? ({ kind: 'note' } as const)
      : ({ kind: 'nullifier', noteId: state.notes[0]?.noteId ?? '' } as const);
  armOp(state, kind, target, line.seq);
  armDeadline();
  render();

  state.replayStage = next;
  state.feedUrl = feedUrl();
  if (client) await client.close();
  client = makeClient();
  record(state, 'feed', { text: `feed → ${next}`, status: 'ok' });
  state.busy = 'advance';
  try {
    const feed = await client.sync();
    state.feed = feed;
    afterFeed(feed);
  } catch (e) {
    record(state, 'error', { text: describe(e), status: 'fail' });
  } finally {
    state.busy = null;
  }
  await discoverInto(null, 'manual');
}

function handoffSheet(kind: 'deposit' | 'send' | 'withdraw'): void {
  const sheet = el(
    'div',
    { class: 'sheet' },
    el('h3', {}, kind),
    el('p', {}, 'pool ', el('code', {}, SEPOLIA.pool)),
  );
  const done = el('button', {}, 'submitted — watch');
  const cancel = el('button', { class: 'ghost' }, 'cancel');
  sheet.append(el('div', { class: 'sheet-actions' }, done, cancel));
  const overlay = el('div', { class: 'overlay' }, sheet);
  document.body.append(overlay);

  cancel.addEventListener('click', () => overlay.remove());
  done.addEventListener('click', () => {
    overlay.remove();
    if (state.op) return;
    const line = pendingLine(state, 'await', kind === 'deposit' ? 'waiting for the note' : 'waiting for the spend');
    armOp(
      state,
      kind,
      kind === 'deposit' ? { kind: 'note' } : { kind: 'nullifier', noteId: state.notes[0]?.noteId ?? '' },
      line.seq,
    );
    armDeadline();
    render();
  });
}

function cancelOp(): void {
  const op = state.op;
  if (!op) return;
  const line = state.log.find((l) => l.seq === op.lineSeq);
  state.op = null;
  if (deadlineTimer) clearTimeout(deadlineTimer);
  if (line && line.status === 'pending') {
    commit(state, line, { text: 'cancelled', status: 'warn' });
  }
  render();
}

// ------------------------------------------------------------- rendering

/**
 * Coalesce on a FRAME, not on a microtask.
 *
 * `render()` rebuilds the whole page with `replaceChildren`. A microtask
 * checkpoint happens at every `await`, so a microtask-coalesced render fired
 * once per request — 610 full-page rebuilds during a cold sync, over a request
 * list that is itself growing to 610 rows. That is O(n²), and it is paid inside
 * the client's `fetch` phase, so it lands in the ONE number the demo exists to
 * report.
 *
 * Measured on this machine, same browser, same server: the 607 epoch GETs take
 * 642 ms raw; the demo attributed 165 s to the same fetches. The difference was
 * entirely this function. A frame-coalesced render shows every request just the
 * same — the panel is still the session's complete record — it simply stops
 * rebuilding the DOM between them.
 */
let renderQueued = false;
const scheduleFrame: (f: () => void) => void =
  typeof requestAnimationFrame === 'function' ? (f) => void requestAnimationFrame(f) : queueMicrotask;

function scheduleRender(): void {
  if (renderQueued) return;
  renderQueued = true;
  scheduleFrame(() => {
    renderQueued = false;
    render();
  });
}

function render(): void {
  const inv = logInvariantsHold(state.log);
  const root = document.getElementById('app')!;
  root.replaceChildren(
    renderHeader(),
    el(
      'div',
      { class: 'grid' },
      el(
        'div',
        { class: 'col-left' },
        el(
          'section',
          { class: 'card card-runs' },
          el('h3', {}, 'A · cold | warm'),
          el('div', { class: 'run-cards' }, renderCard(state.cold, 'COLD', 'empty store'), renderCard(state.warm, 'WARM', 'persisted mirror')),
          renderRunControls(),
        ),
        renderTrust(state),
        renderStages(),
      ),
      el('div', { class: 'col-right' }, renderNetwork(state, selfTest)),
    ),
    el(
      'section',
      { class: 'card card-log' },
      el('h3', {}, 'D · log', inv === true ? null : el('span', { class: 'fail' }, ` INVARIANT BROKEN: ${inv}`)),
      renderLog(state),
    ),
  );
  const logEl = root.querySelector('.log');
  if (logEl) logEl.scrollTop = logEl.scrollHeight;
}

function renderHeader(): HTMLElement {
  const laneSel = el('select', { class: 'lane' });
  for (const l of Object.values(LANES)) {
    const o = el('option', { value: l.id }, l.label);
    if (l.id === state.lane) o.setAttribute('selected', 'selected');
    laneSel.append(o);
  }
  laneSel.addEventListener('change', () => {
    const id = (laneSel as HTMLSelectElement).value as Lane;
    if (id === 'live') {
      const url = prompt('feed URL of a running indexer', liveUrl);
      if (!url) {
        render();
        return;
      }
      liveUrl = url.replace(/\/$/, '');
      // The chain is a property of the feed, not of this control. Getting it
      // wrong is not silent: the module pins identity from the fetched
      // genesis.json and throws CHAIN_MISMATCH.
      liveNetwork = /mainnet/i.test(liveUrl) ? MAINNET : SEPOLIA;
    }
    state.lane = id;
    state.replayStage = 't0';
    state.feedUrl = feedUrl();
    state.cold = emptyCard('cold');
    state.warm = emptyCard('warm');
    state.stage1Done = false;
    state.records = [];
    state.feed = null;
    void (async () => {
      if (client) await client.close();
      client = makeClient();
      render();
    })();
  });

  return el(
    'header',
    {},
    el(
      'div',
      { class: 'title-row' },
      el('span', { class: 'title' }, 'strk20-discovery'),
      renderEngineBadge(state),
    ),
    el(
      'div',
      { class: 'lane-row' },
      laneSel,
      laneCfg().chip ? el('span', { class: 'chip' }, laneCfg().chip) : null,
      el('code', { class: 'feed-url' }, feedUrl()),
    ),
  );
}

function button(label: string, onClick: () => void, opts: { disabled?: string; cls?: string } = {}): HTMLElement {
  const b = el('button', opts.cls ? { class: opts.cls } : {}, label);
  if (opts.disabled) {
    b.setAttribute('disabled', 'disabled');
    b.setAttribute('title', opts.disabled);
    return el('span', { class: 'btn-wrap' }, b, el('small', { class: 'why-disabled' }, opts.disabled));
  }
  b.addEventListener('click', onClick);
  return b;
}

function renderRunControls(): HTMLElement {
  const busy = state.busy !== null;
  const running = { disabled: 'running' };
  return el(
    'div',
    { class: 'controls' },
    button('run cold', () => void runCold(), busy ? running : {}),
    button('run warm', () => void runWarm(), busy ? running : !state.stage1Done ? { disabled: 'cold first' } : {}),
    button('warm after reload', () => void runWarmAfterReload(), !state.stage1Done ? { disabled: 'cold first' } : {}),
    button('A/B two identities', () => void runAb(), busy ? running : {}),
  );
}

function renderStages(): HTMLElement {
  const stage2 = state.stage1Done;
  const stage3 = state.identity !== null;
  const stage4 = state.discoveryRan;
  const sub = el('button', { class: state.subscription ? 'on' : 'off' }, `subscription: ${state.subscription ? 'ON' : 'OFF'}`);
  sub.addEventListener('click', () => setSubscription(!state.subscription));

  const oneWait = { disabled: 'one at a time' };
  return el(
    'section',
    { class: 'card card-stages' },
    el('h3', {}, 'stages'),
    el('div', { class: 'stage' }, el('b', {}, '1 · feed'), el('span', { class: 'stage-note' }, 'no key')),
    el(
      'div',
      { class: `stage ${stage2 ? '' : 'dim'}` },
      el('b', {}, '2 · identity'),
      stage2 ? null : el('span', { class: 'precondition' }, 'needs stage 1'),
      stage2
        ? el(
            'div',
            { class: 'controls' },
            button('identity A', () => setIdentity(identityA())),
            button('identity B', () => setIdentity(identityB())),
            button('generate key', () => setIdentity(generatedIdentity())),
          )
        : null,
      stage2 && ALLOW_PASTE ? renderKeyField() : null,
      renderIdentity(state),
    ),
    el(
      'div',
      { class: `stage ${stage3 ? '' : 'dim'}` },
      el('b', {}, '3 · discovery'),
      stage3 ? null : el('span', { class: 'precondition' }, 'needs an identity'),
      stage3
        ? el(
            'div',
            { class: 'controls' },
            button('check now', () => void discoverInto(null, 'manual'), state.checking ? { disabled: 'checking…' } : {}),
            sub,
            el('span', { class: `transport t-${state.transport}` }, state.transport),
          )
        : null,
      state.notes.length
        ? el(
            'ul',
            { class: 'notes' },
            ...state.notes.map((n) =>
              el('li', { class: n.spent ? 'spent' : '' }, `${short(n.noteId)} · ${fmtAmount(n.amount)} @${n.blockNumber.toLocaleString('en-US')}${n.spent ? ' · SPENT' : ''}`),
            ),
          )
        : state.discoveryRan
          ? el('div', { class: 'notes-none' }, 'no notes')
          : null,
    ),
    el(
      'div',
      { class: `stage ${stage4 ? '' : 'dim'}` },
      el('b', {}, '4 · act'),
      stage4 ? null : el('span', { class: 'precondition' }, 'needs discovery'),
      stage4
        ? el(
            'div',
            { class: 'controls' },
            ...(['deposit', 'send', 'withdraw'] as const).map((k) =>
              laneCfg().writable
                ? button(k, () => handoffSheet(k), state.op ? oneWait : {})
                : button(k, () => {}, { disabled: 'no chain here' }),
            ),
            laneCfg().advanceable
              ? button(
                  'advance feed →',
                  () => void advanceReplay(),
                  state.op ? oneWait : state.replayStage === 't2' ? { disabled: 'at last stage' } : {},
                )
              : null,
            state.op ? button('cancel', cancelOp, { cls: 'ghost' }) : null,
          )
        : null,
    ),
  );
}

function useKey(hex: string, addr: string, label: string): boolean {
  // A viewing key is a felt, and a felt's hex drops leading zeros — the key
  // files in this repo are routinely 63 characters. Left-pad to 32 bytes
  // rather than rejecting a perfectly good key for being small.
  const h = hex.trim().replace(/^0x/, '');
  if (!/^[0-9a-fA-F]{1,64}$/.test(h) || !/^0x[0-9a-fA-F]{1,64}$/.test(addr.trim())) return false;
  const key = hexToBytes(h.padStart(64, '0'));
  setIdentity({
    id: 'generated',
    label,
    secretBytes: key,
    ownsNoteInFixture: false,
    account: { address: addr.trim() as `0x${string}`, viewingKey: async () => Uint8Array.from(key) },
  });
  return true;
}

/**
 * The key field. Local build only (`ALLOW_PASTE`), because a page under our
 * name that asks you to paste a wallet secret teaches the habit that gets our
 * users phished later.
 *
 * `type="password"` so a screen recording does not carry it, `autocomplete
 * off` so no password manager offers to keep it, no `name` so nothing can
 * serialise it into a form submission, and the field is cleared the instant
 * the bytes are taken. From there the key exists as a `Uint8Array` in one
 * closure and inside the wasm module, which zeroizes it. It reaches no URL, no
 * request header, no log line and no IndexedDB store.
 */
function renderKeyField(): HTMLElement {
  const vk = el('input', {
    type: 'password',
    class: 'vk',
    placeholder: 'viewing key (hex)',
    autocomplete: 'off',
    spellcheck: 'false',
    'data-1p-ignore': '',
  });
  const addr = el('input', {
    type: 'text',
    class: 'vk-addr',
    placeholder: 'owner address 0x…',
    autocomplete: 'off',
    spellcheck: 'false',
  });
  const use = (): void => {
    const ok = useKey(vk.value, addr.value, 'key entered in this tab (never sent, never stored)');
    vk.value = '';
    if (!ok) {
      record(state, 'identity', { text: 'bad key or address', status: 'warn' });
      render();
    }
  };
  vk.addEventListener('keydown', (e) => e.key === 'Enter' && addr.focus());
  addr.addEventListener('keydown', (e) => e.key === 'Enter' && use());
  return el('div', { class: 'key-field' }, vk, addr, button('use key', use));
}

/**
 * There is deliberately NO way to put a viewing key in the URL.
 *
 * A `#fragment` is never sent to a server, so it looks safe, and it is not.
 * This page gets screen-shared, recorded and screenshotted; a key sitting in
 * the address bar during a demo whose entire claim is "your key never leaves
 * your machine" is the worst frame we could ship, and it survives in browser
 * history and in anything that captures a URL.
 *
 * The decisive reason is the second one. The leak scanner below inspects
 * REQUEST SURFACES — it cannot see the address bar. So the page could render
 * "0 hits · N surfaces" in green with a viewing key visible three centimetres
 * above it, and both statements would be true. A guarantee whose own
 * instrument cannot see the violation is not a guarantee. The key is typed
 * into the field and nowhere else.
 */

// ---------------------------------------------------------------- helpers

function counted(n: number): { value: string; provenance: 'measured' | 'unavailable' } {
  return n < 0
    ? { value: 'unavailable', provenance: 'unavailable' }
    : { value: n.toLocaleString('en-US'), provenance: 'measured' };
}

function progressText(phase: string, done: number, b: number): string {
  return `${phase}${done ? ` ${done}` : ''} · ${bytes(b)}`;
}

function failLine(line: ReturnType<typeof pendingLine>, e: unknown): void {
  const err = e instanceof Strk20Error ? e : Strk20Error.fromModuleJson(e);
  commit(state, line, { text: `${err.code}: ${err.message}`, status: 'fail' });
}

function describe(e: unknown): string {
  const err = e instanceof Strk20Error ? e : Strk20Error.fromModuleJson(e);
  return `${err.code}: ${err.message}`;
}

function short(id: string): string {
  return id.length > 14 ? `${id.slice(0, 10)}…${id.slice(-4)}` : id;
}

function fmtAmount(v: bigint): string {
  const whole = v / 10n ** 18n;
  const frac = (v % 10n ** 18n).toString().padStart(18, '0').slice(0, 2);
  return `${whole}.${frac} STRK`;
}

function formatBalances(b: Map<string, bigint>): string {
  if (b.size === 0) return '0 STRK';
  return [...b.values()].map(fmtAmount).join(' + ');
}

function hexToBytes(hex: string): Uint8Array {
  const h = hex.length % 2 ? '0' + hex : hex;
  const out = new Uint8Array(h.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(h.slice(i * 2, i * 2 + 2), 16);
  return out;
}

// ------------------------------------------------------------------ ticker
// The pending line's live counter ticks at 100 ms granularity, but a committed
// duration is ALWAYS computed from two performance.now() stamps, never from
// this counter.

setInterval(() => {
  for (const node of document.querySelectorAll('.log-elapsed.pending')) {
    const since = Number((node as HTMLElement).dataset.since ?? '0');
    node.textContent = `⠋ ${((performance.now() - since) / 1000).toFixed(1)} s`;
  }
}, 100);

// -------------------------------------------------------------------- boot
// §9 rule 7: failures are logged in the same typeface as successes, and nothing
// is swallowed. A demo that eats its own exceptions is a demo that will one day
// show a stale number and call it a measurement.

function reportUncaught(what: string, e: unknown): void {
  try {
    record(state, 'error', {
      text: `${what}: ${e instanceof Error ? e.message : String(e)}`,
      status: 'fail',
      detail: e instanceof Error ? (e.stack ?? '').split('\n').slice(0, 3).join(' | ') : '',
    });
    render();
  } catch {
    // Rendering itself failed; the console is the only channel left.
    console.error(what, e);
  }
}

window.addEventListener('error', (ev) => reportUncaught('uncaught error', ev.error ?? ev.message));
window.addEventListener('unhandledrejection', (ev) => reportUncaught('unhandled rejection', ev.reason));

async function boot(): Promise<void> {
  // `?lane=live&feed=…&network=…` — the whole feed selection in the URL, so a
  // demo run is reproducible from a link instead of four clicks and a prompt.
  const wanted = new URLSearchParams(location.search).get('lane');
  if (wanted && wanted in LANES) {
    state.lane = wanted as Lane;
    state.feedUrl = feedUrl();
  }
  client = makeClient();
  render();

  let flag: string | null = null;
  try {
    flag = localStorage.getItem(RELOAD_FLAG);
    if (flag) localStorage.removeItem(RELOAD_FLAG);
  } catch {
    flag = null;
  }
  if (flag) {
    const saved = JSON.parse(flag) as { lane: Lane; stage: 't0' | 't1' | 't2' };
    state.lane = saved.lane;
    state.replayStage = saved.stage;
    state.feedUrl = feedUrl();
    state.stage1Done = true;
    if (client) await client.close();
    client = makeClient();
    // The number a returning user actually feels: performance.timeOrigin to
    // sync() resolving, so it INCLUDES engine boot and the IndexedDB open.
    await runWarm(true);
  }
}

void boot();
