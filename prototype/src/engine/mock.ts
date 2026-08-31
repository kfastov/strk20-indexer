/**
 * The one and only implementation of `Strk20Engine` today.
 *
 * It fetches nothing, decompresses nothing, verifies nothing and discovers
 * nothing. It sleeps for jittered intervals shaped like the phases the real
 * client will have, and it reports the wall clock it actually spent asleep.
 * So the durations are real measurements of a fake process — which is exactly
 * as trustworthy as it sounds, and why `info.simulated` is true and the banner
 * is not dismissible.
 *
 * When `wasm.ts` lands it implements the same interface over fzstd + a
 * wasm-bindgen module and `main.ts` changes one line.
 */

import {
  ANCHORS_BYTES,
  CHAIN_ID,
  EPOCH_COUNT,
  FIRST_EPOCH,
  GENESIS_BYTES,
  HEAD_BYTES,
  LAST_EPOCH,
  MANIFEST_BYTES,
  POOL,
  SNAPSHOT_EPOCH,
  epochBytes,
  epochPath,
  snapshotAnchorPath,
  snapshotPath,
} from './fixtures';
import { Latency, sleep, sleepProgress } from './latency';
import type { SimulatedChain } from './chain';
import type {
  ColdStartLane,
  DiscoverOut,
  EngineEvent,
  FeedState,
  Identity,
  NetworkRecord,
  Note,
  OpenResult,
  Phase,
  PhaseName,
  PhaseProgress,
  Strk20Engine,
  SyncRequest,
  SyncResult,
  SyncTiming,
  Unsubscribe,
} from './types';

interface LocalState {
  /** The folded mirror exists for this key. */
  folded: boolean;
  /** Analogue of the sealed AEAD state blob: what we saw last pass. */
  seenNotes: Set<string>;
  seenSpent: Set<string>;
}

/**
 * Records one phase, sleeping for a jittered duration and timing it for real.
 *
 * It reports the phase to the host the moment it STARTS, which is what lets the
 * log's mutating line follow the sync phase by phase: the host sees the phase
 * change, commits the previous pending line with its measured elapsed, and opens
 * a new one.
 */
class PhaseRecorder {
  private phases: Phase[] = [];
  private startedAt = performance.now();

  constructor(private report?: (p: PhaseProgress) => void) {}

  async run(
    name: PhaseName,
    ms: number,
    opts: {
      detail?: string | undefined;
      signal?: AbortSignal | undefined;
      onTick?: ((fraction: number) => void) | undefined;
      progress?: boolean | undefined;
    } = {},
  ): Promise<void> {
    const t0 = performance.now();
    this.report?.({ phase: name, detail: opts.detail ?? '', fraction: 0 });
    if (opts.progress && opts.onTick) {
      await sleepProgress(ms, (f) => opts.onTick?.(f), opts.signal);
    } else {
      await sleep(ms, opts.signal);
    }
    const phase: Phase = { name, ms: performance.now() - t0 };
    this.phases.push(opts.detail === undefined ? phase : { ...phase, detail: opts.detail });
  }

  skip(name: PhaseName, bytes: number, detail: string): void {
    this.phases.push({ name, ms: 0, skipped: true, skippedBytes: bytes, detail });
  }

  finish(): SyncTiming {
    return { totalMs: performance.now() - this.startedAt, phases: this.phases };
  }
}

export class MockEngine implements Strk20Engine {
  readonly info = {
    name: 'mock-engine',
    build: 'prototype/0.1 (no wasm, no network, no crypto)',
    simulated: true,
    notice: 'sleeps instead of syncing; every duration is a timed sleep',
  } as const;

  private lat: Latency;
  private records: NetworkRecord[] = [];
  private local: Record<'A' | 'B', LocalState>;
  private _lane: ColdStartLane = 'epochs';
  private booted = false;
  private coldRuns = 0;
  /**
   * What the CLIENT knows about the head — not what the chain knows.
   * With the subscription off, this only moves when a sync or a manual check
   * runs, because with no stream open the client genuinely learns nothing.
   */
  private observedHead = 0;
  private observedL1 = 0;

  constructor(private chain: SimulatedChain, seed: number) {
    this.lat = new Latency(seed);
    this.local = { A: blankLocal(), B: blankLocal() };
    this.observedHead = chain.head;
    this.observedL1 = chain.l1Accepted;
  }

  /** Prototype-only affordance; see `EngineProbe` in ./types. */
  readonly probe = {
    plantKeyProbe: (identity: Identity): void => {
      const url = `/v1/sync/state?viewing_key=${identity.viewingKey}&address=${identity.address}`;
      this.records.push({
        method: 'GET',
        url,
        status: 200,
        bytes: 4_112,
        ms: 41,
        source: 'network',
        synthetic: 'planted by the scanner self-test — this is compat mode, not the feed',
      });
    },
    clearProbes: (): void => {
      this.records = this.records.filter((r) => r.synthetic === undefined);
    },
  };

  // -------------------------------------------------------------------------
  // lifecycle
  // -------------------------------------------------------------------------

  async open(identity: Identity): Promise<OpenResult> {
    const rec = new PhaseRecorder(undefined);
    if (!this.booted) {
      await rec.run('boot', this.lat.draw({ centre: 180, spread: 0.3 }));
      this.booted = true;
    }
    await rec.run('open', this.lat.draw({ centre: 14, spread: 0.6 }));
    return {
      warm: this.local[identity.id].folded,
      store: 'memory',
      persisted: false,
      timing: rec.finish(),
    };
  }

  async close(): Promise<void> {
    this.records = [];
  }

  // -------------------------------------------------------------------------
  // the cold / warm path
  // -------------------------------------------------------------------------

  async sync(req: SyncRequest): Promise<SyncResult> {
    const { identity, mode, signal, onPhase } = req;
    const local = this.local[identity.id];

    if (mode === 'cold') {
      await this.clearLocalState(identity);
      this.coldRuns++;
    }

    const rec = new PhaseRecorder(onPhase);
    const cached = mode === 'cold' && this.coldRuns > 1;

    if (mode === 'cold' && this._lane === 'epochs') {
      await this.coldEpochLane(rec, signal, onPhase, cached);
    } else if (mode === 'cold') {
      await this.coldSnapshotLane(rec, signal, onPhase);
    } else {
      await this.warmLane(rec, signal);
    }

    this.observe();
    const discovered = await this.runDiscovery(rec, identity, signal);

    if (mode === 'cold') {
      await rec.run('export', this.lat.draw({ centre: 110, spread: 0.4 }));
    }

    local.folded = true;
    return {
      ...discovered,
      timing: rec.finish(),
      feed: this.feed(),
      mode,
      lane: this._lane,
    };
  }

  private async coldEpochLane(
    rec: PhaseRecorder,
    signal: AbortSignal | undefined,
    onPhase: SyncRequest['onPhase'],
    cached: boolean,
  ): Promise<void> {
    await rec.run('plan', this.lat.draw({ centre: 38, spread: 0.5 }), {
      signal,
    });

    this.push('/feed/genesis.json', GENESIS_BYTES, 200, 'network');
    this.push('/feed/manifest.json', MANIFEST_BYTES, 200, 'network');

    // The browser HTTP cache legitimately serves immutable epoch files on a
    // second cold run. A page cannot clear it, so pretending otherwise would be
    // the easiest lie in the whole prototype.
    const fetchMs = this.lat.draw({
      centre: cached ? 820 : 2_400,
      spread: 0.22,
      tailChance: 0.2,
      tailFactor: 1.6,
    });
    let pushed = 0;
    let runBytes = 0;
    await rec.run('fetch', fetchMs, {
      signal,
      progress: true,
      onTick: (f) => {
        const want = Math.floor(f * EPOCH_COUNT);
        while (pushed < want) {
          const e = FIRST_EPOCH + pushed;
          runBytes += epochBytes(e);
          this.push(epochPath(e), epochBytes(e), 200, cached ? 'http-cache' : 'network');
          pushed++;
        }
        onPhase?.({
          phase: 'fetch',
          fraction: f,
          // Bytes for THIS run, not the session total — the session total is the
          // requests panel's job and putting it here read as a per-run figure.
          detail: `epoch ${Math.min(LAST_EPOCH, FIRST_EPOCH + pushed)} / ${LAST_EPOCH} · ${fmtMb(runBytes)}`,
        });
      },
    });
    while (pushed < EPOCH_COUNT) {
      const e = FIRST_EPOCH + pushed;
      this.push(epochPath(e), epochBytes(e), 200, cached ? 'http-cache' : 'network');
      pushed++;
    }
    this.push('/feed/head.ndjson', HEAD_BYTES, 200, 'network');

    await rec.run('inflate', this.lat.draw({ centre: 880, spread: 0.25 }), {
      signal,
      progress: true,
      onTick: (f) => onPhase?.({ phase: 'inflate', fraction: f, detail: '' }),
    });

    await rec.run('verify+fold', this.lat.draw({ centre: 2_280, spread: 0.2 }), {
      signal,
      progress: true,
      onTick: (f) =>
        onPhase?.({
          phase: 'verify+fold',
          fraction: f,
          detail: `${Math.floor(f * EPOCH_COUNT)} / ${EPOCH_COUNT}`,
        }),
    });
  }

  private async coldSnapshotLane(
    rec: PhaseRecorder,
    signal: AbortSignal | undefined,
    onPhase: SyncRequest['onPhase'],
  ): Promise<void> {
    await rec.run('plan', this.lat.draw({ centre: 30, spread: 0.5 }), {
      signal,
    });

    this.push('/feed/genesis.json', GENESIS_BYTES, 200, 'network');
    this.push('/feed/manifest.json', MANIFEST_BYTES, 200, 'network');

    await rec.run('fetch', this.lat.draw({ centre: 420, spread: 0.3, tailChance: 0.2 }), {
      signal,
      progress: true,
      onTick: (f) => onPhase?.({ phase: 'fetch', fraction: f, detail: '' }),
    });
    // Byte counts here are unknown: no snapshot has ever been cut. Recorded as
    // zero and rendered as "?" so nobody reads an invented size as a size.
    this.push(snapshotPath(SNAPSHOT_EPOCH), 0, 200, 'network');
    this.push(snapshotAnchorPath(SNAPSHOT_EPOCH), 0, 200, 'network');
    this.push('/feed/head.ndjson', HEAD_BYTES, 200, 'network');

    await rec.run('snapshot', this.lat.draw({ centre: 260, spread: 0.3 }), {
      signal,
    });
  }

  private async warmLane(rec: PhaseRecorder, signal: AbortSignal | undefined): Promise<void> {
    await rec.run('plan', this.lat.draw({ centre: 4, spread: 0.6 }), {
      signal,
    });
    this.push('/feed/head.ndjson', 0, 304, 'network');

    rec.skip('fetch', this.totalFeedBytes(), 'skipped');
    rec.skip('inflate', 0, 'skipped');
    rec.skip('verify+fold', 0, 'skipped');

    await rec.run('load', this.lat.draw({ centre: 9, spread: 0.5 }), {
      signal,
    });
    await rec.run('apply', this.lat.draw({ centre: 3, spread: 0.6 }), { signal });
  }

  // -------------------------------------------------------------------------
  // discovery
  // -------------------------------------------------------------------------

  async discover(identity: Identity, signal?: AbortSignal): Promise<DiscoverOut> {
    this.observe();
    const rec = new PhaseRecorder(undefined);
    // A poke means new tail bytes. The real client refetches head.ndjson and
    // re-runs the pass; that ongoing request cost is visible in the panel.
    this.push('/feed/head.ndjson', this.lat.between(180, 900), 200, 'network');
    await rec.run('apply', this.lat.draw({ centre: 6, spread: 0.6 }), { signal });
    return this.runDiscovery(rec, identity, signal, true);
  }

  private async runDiscovery(
    rec: PhaseRecorder,
    identity: Identity,
    signal: AbortSignal | undefined,
    incremental = false,
  ): Promise<DiscoverOut> {
    const ms = incremental
      ? this.lat.draw({ centre: 22, spread: 0.5 })
      : this.lat.draw({ centre: 330, spread: 0.3 });
    await rec.run('discover', ms, {
      signal,
    });

    const notes = this.chain.notesFor(identity);
    const local = this.local[identity.id];
    const added: Note[] = [];
    const spent: Note[] = [];
    for (const n of notes) {
      if (!local.seenNotes.has(n.id)) {
        added.push(n);
        local.seenNotes.add(n.id);
      }
      if (n.spent && !local.seenSpent.has(n.nullifier)) {
        spent.push(n);
        local.seenSpent.add(n.nullifier);
      }
    }
    return { notes, added, spent, timing: rec.finish() };
  }

  // -------------------------------------------------------------------------
  // network evidence
  // -------------------------------------------------------------------------

  network(): readonly NetworkRecord[] {
    return this.records;
  }

  private push(url: string, bytes: number, status: number, source: NetworkRecord['source']): void {
    this.records.push({
      method: 'GET',
      url,
      status,
      bytes: Math.round(bytes),
      ms: Math.round(this.lat.draw({ centre: source === 'http-cache' ? 1.4 : 11, spread: 0.7 })),
      source,
    });
  }

  private totalFeedBytes(): number {
    let sum = GENESIS_BYTES + MANIFEST_BYTES + HEAD_BYTES + ANCHORS_BYTES;
    for (let e = FIRST_EPOCH; e <= LAST_EPOCH; e++) sum += epochBytes(e);
    return sum;
  }

  // -------------------------------------------------------------------------
  // local state, lanes, feed
  // -------------------------------------------------------------------------

  async clearLocalState(identity: Identity): Promise<void> {
    this.local[identity.id] = blankLocal();
    await sleep(this.lat.draw({ centre: 9, spread: 0.5 }));
  }

  /**
   * The subscription IS how a client learns the head moved. While nobody is
   * subscribed the observed head stays where the last sync left it — which is
   * the truthful behaviour and makes the toggle mean something in the panel.
   */
  subscribe(handler: (ev: EngineEvent) => void): Unsubscribe {
    return this.chain.onEvent((ev) => {
      if (ev.type === 'head') {
        this.observedHead = ev.head;
        this.observedL1 = ev.l1Accepted;
      }
      handler(ev);
    });
  }

  private observe(): void {
    this.observedHead = this.chain.head;
    this.observedL1 = this.chain.l1Accepted;
  }

  setLane(lane: ColdStartLane): void {
    this._lane = lane;
  }

  lane(): ColdStartLane {
    return this._lane;
  }

  feed(): FeedState {
    return {
      chainId: CHAIN_ID,
      pool: POOL,
      head: this.observedHead,
      l1Accepted: this.observedL1,
      latestEpoch: LAST_EPOCH,
      epochCount: EPOCH_COUNT,
      feedBytes: this.totalFeedBytes(),
      verified: this._lane === 'snapshot' ? 'server-asserted' : 'replayed',
      // Real feeds today: manifest.snapshot === null. Roadmap item 1.
      snapshotAvailable: false,
    };
  }
}

function blankLocal(): LocalState {
  return { folded: false, seenNotes: new Set(), seenSpent: new Set() };
}

function fmtMb(bytes: number): string {
  return `${(bytes / 1e6).toFixed(1)} MB`;
}
