/**
 * `KeylessClient` — §4.3's data flow, driving §3.3's trampoline.
 *
 * Nothing about the fetch plan is decided here. The wrapper GETs the paths a
 * key-blind module named, inflates within the cap the module named, and hands
 * both buffers back. That is the whole reason the module owns `request_log()`:
 * the component that authors the URLs cannot see a key.
 *
 * Divergences from §4.2 in THIS build, each raised as CONFIG_INVALID rather
 * than silently downgraded (§4.5's rule: a caller that asked for something and
 * got nothing should learn it at construction, not from a latency graph):
 *   - `worker: true` is not built. Everything runs on the caller's thread and
 *     `status().blocking` is true. §4.11's worker, SSE leader election and the
 *     `close()`-frees-linear-memory property all wait on the wasm engine.
 *   - `coldStart: 'snapshot'` is accepted but no feed publishes a snapshot yet
 *     (roadmap item 1), so `auto` resolves to the epochs lane inside the module.
 *   - `anchorRpcUrl` / `anchorPolicy: 'require'` are rejected: no engine here
 *     emits a `Step::Rpc`, and accepting the option would imply a verification
 *     grade we cannot reach.
 */

import { Decompress } from 'fzstd';
import { assertKey, zeroize } from './account.ts';
import { Strk20Error } from './errors.ts';
import type {
  DiscoverStepOut,
  Engine,
  EngineFactory,
  EngineInfo,
  ResponseEnvelope,
  Step,
  StepDone,
  StepFetch,
} from './engine.ts';
import { keyId as deriveKeyId } from './kdf.ts';
import {
  cacheRecord,
  openLive,
  request,
  resolveFetch,
  type FetchLike,
  type LiveStream,
  type NetContext,
} from './net.ts';
import { resolveProfile } from './profiles.ts';
import { sha256Hex } from './sha256.ts';
import { dbName, deleteDatabase, openStorage, type StateMeta, type StorageAdapter } from './storage.ts';
import type {
  Account,
  ChainProfile,
  ClientStatus,
  DiscoveryClient,
  DiscoveryEvent,
  DiscoveryProvider,
  FeedState,
  HistoryTx,
  NetworkSummary,
  Note,
  NotesResult,
  Phase,
  Progress,
  RequestRecord,
  Subscription,
  SyncTiming,
} from './types.ts';

export interface KeylessClientOptions {
  feedUrl: string;
  network?: 'mainnet' | 'sepolia' | ChainProfile;
  coldStart?: 'auto' | 'snapshot' | 'epochs';
  persistence?: 'indexeddb' | 'memory' | StorageAdapter;
  persist?: 'raw' | 'folded' | 'both';
  live?: boolean;
  pollIntervalMs?: number;
  worker?: boolean;
  prefetchConcurrency?: number;
  stepBudgetMs?: number;
  maxArtifactBytes?: number;
  anchorRpcUrl?: string;
  anchorPolicy?: 'off' | 'best-effort' | 'require';
  requestPersistentStorage?: boolean;
  wasmUrl?: string | URL;
  fetch?: FetchLike;
  onRequest?: (r: RequestRecord) => void;
  /**
   * NOT in §4.2. The engine seam, so the demo can run today on the mock and
   * switch to wasm by changing one binding. Defaults to the wasm factory, which
   * fails loudly when the module is not built — never to the mock, because a
   * silent fallback is how a screenshot ends up misattributing a number.
   */
  engine: EngineFactory;
  /**
   * NOT in §4.2, and it should be. demo-app.md §7 rule 1 requires the two runs
   * of the A/B comparison to start "in separate database-name suffixes" — and
   * §9.1 forbids letting identity B share identity A's IndexedDB, which would
   * make its "cold" run warm and its request list short. §4.2's constructor
   * offers no way to say that, so the requirement is unimplementable as
   * written. Added here; recorded as a spec gap.
   */
  databaseSuffix?: string;
}

const ZERO_PHASES: SyncTiming['phases'] = {
  open: 0,
  manifest: 0,
  fetch: 0,
  decompress: 0,
  apply: 0,
  load: 0,
  export: 0,
  anchor: 0,
  discover: 0,
};

interface Staged {
  compressed: Uint8Array;
  status: number;
  etag: string | null;
  source: RequestRecord['source'];
}

export class KeylessClient implements DiscoveryClient {
  readonly #opts: Required<Pick<KeylessClientOptions, 'feedUrl' | 'coldStart' | 'persist' | 'live' | 'pollIntervalMs' | 'prefetchConcurrency' | 'stepBudgetMs' | 'maxArtifactBytes'>>;
  readonly #profile: ChainProfile;
  readonly #engineFactory: EngineFactory;
  readonly #persistence: 'indexeddb' | 'memory' | StorageAdapter;
  readonly #requestPersistentStorage: boolean;
  readonly #dbSuffix: string;
  readonly #onRequest: ((r: RequestRecord) => void) | undefined;
  readonly #net: NetContext;

  #storage: StorageAdapter | null = null;
  #engine: Engine | null = null;
  #records: RequestRecord[] = [];
  #busy: Promise<unknown> = Promise.resolve();
  #closed = false;
  #persisted = false;
  #fromCache: SyncTiming['fromCache'] = 'none';
  #lastFeed: FeedState | null = null;
  #accounts = new Set<string>();
  #transport: 'sse' | 'polling' = 'polling';
  #openMs = 0;
  #loadMs = 0;
  #boot: { engineCreatedMs: number } | null = null;

  constructor(opts: KeylessClientOptions) {
    if (!opts.feedUrl) throw cfg('feedUrl', 'a feed URL is required');
    if (opts.worker === true) {
      throw cfg('worker', 'the worker host is not built in this tree; pass worker:false explicitly', {
        got: true,
        built: false,
      });
    }
    if (opts.anchorRpcUrl || opts.anchorPolicy === 'require') {
      throw cfg('anchorPolicy', 'no engine in this tree emits a Step::Rpc, so `anchored` is unreachable', {
        got: opts.anchorPolicy ?? 'best-effort',
        built: 'off',
      });
    }
    if (opts.persist && !['raw', 'folded', 'both'].includes(opts.persist)) {
      throw cfg('persist', 'unknown persist mode', { got: String(opts.persist) });
    }
    if (!opts.engine) throw cfg('engine', 'an engine factory must be bound explicitly');

    this.#profile = resolveProfile(opts.network);
    this.#engineFactory = opts.engine;
    this.#persistence = opts.persistence ?? 'indexeddb';
    this.#requestPersistentStorage = opts.requestPersistentStorage ?? false;
    this.#dbSuffix = opts.databaseSuffix ? `:${opts.databaseSuffix}` : '';
    this.#onRequest = opts.onRequest;
    this.#opts = {
      feedUrl: opts.feedUrl.replace(/\/$/, ''),
      coldStart: opts.coldStart ?? 'auto',
      persist: opts.persist ?? 'both',
      live: opts.live ?? true,
      pollIntervalMs: opts.pollIntervalMs ?? 30_000,
      prefetchConcurrency: opts.prefetchConcurrency ?? 6,
      stepBudgetMs: opts.stepBudgetMs ?? 16,
      maxArtifactBytes: opts.maxArtifactBytes ?? 64 * 2 ** 20,
    };
    this.#net = {
      fetchImpl: resolveFetch(opts.fetch),
      now: () => nowMs(),
      onRecord: (r) => {
        this.#records.push(r);
        this.#onRequest?.(r);
      },
    };
  }

  get databaseName(): string {
    return dbName(this.#profile.chainId, this.#profile.pool) + this.#dbSuffix;
  }

  get profile(): ChainProfile {
    return this.#profile;
  }

  // ------------------------------------------------------------------ sync

  async sync(opts: { signal?: AbortSignal; onProgress?: (p: Progress) => void } = {}): Promise<FeedState> {
    return this.#serialize(() => this.#syncOnce(opts));
  }

  async #ensureOpen(): Promise<StorageAdapter> {
    if (this.#storage) return this.#storage;
    const t0 = nowMs();
    this.#storage = await openStorage(this.#persistence, this.databaseName);
    if (this.#requestPersistentStorage) {
      const s = (globalThis as { navigator?: Navigator }).navigator?.storage;
      try {
        this.#persisted = (await s?.persist?.()) ?? false;
      } catch {
        this.#persisted = false;
      }
    } else {
      try {
        const s = (globalThis as { navigator?: Navigator }).navigator?.storage;
        this.#persisted = (await s?.persisted?.()) ?? false;
      } catch {
        this.#persisted = false;
      }
    }
    this.#openMs = nowMs() - t0;
    return this.#storage;
  }

  async #ensureEngine(): Promise<Engine> {
    if (this.#engine) return this.#engine;
    const storage = await this.#ensureOpen();
    const profileJson = JSON.stringify(this.#profile);
    const t0 = nowMs();

    if (this.#opts.persist !== 'raw') {
      const stored = await storage.stateGet();
      if (stored) {
        const restored = await this.#engineFactory.load(profileJson, stored.frames);
        if (restored) {
          this.#engine = restored;
          this.#fromCache = 'folded';
          this.#loadMs = nowMs() - t0;
          return restored;
        }
        // §4.5's invalidation table: a blob that will not load is deleted, and
        // we fall through to R, then to the network.
        await storage.stateClear();
      }
    }
    this.#engine = await this.#engineFactory.create(profileJson);
    this.#boot = { engineCreatedMs: nowMs() - t0 };
    this.#fromCache = 'none';
    this.#loadMs = 0;
    return this.#engine;
  }

  async #syncOnce(opts: { signal?: AbortSignal; onProgress?: (p: Progress) => void }): Promise<FeedState> {
    const t0 = nowMs();
    const storage = await this.#ensureOpen();
    const engine = await this.#ensureEngine();
    const phases: SyncTiming['phases'] = { ...ZERO_PHASES, open: this.#openMs, load: this.#loadMs };
    const before = this.#records.length;
    const cold = (JSON.parse(engine.info()) as EngineInfo).last_epoch < 0;

    const staged = new Map<string, Staged>();
    let stepJson = engine.sync_begin(this.#opts.coldStart);
    let epochsSeen = 0;
    let epochsTotal = 0;

    for (;;) {
      if (opts.signal?.aborted) {
        engine.sync_abort();
        throw new Strk20Error('ABORTED', 'sync aborted');
      }
      const step = JSON.parse(stepJson) as Step;

      if (step.step === 'done') {
        const done: StepDone = step;
        if (done.state_dirty && this.#opts.persist !== 'raw') {
          const tp = nowMs();
          await this.#persistFolded(storage, engine);
          phases.export += nowMs() - tp;
        }
        const timing: SyncTiming = {
          totalMs: nowMs() - t0,
          phases,
          cold,
          fromCache: this.#fromCache,
        };
        const info = JSON.parse(engine.info()) as EngineInfo;
        const feed: FeedState = {
          head: done.outcome.head,
          l1Accepted: done.outcome.l1_accepted,
          lastEpoch: info.last_epoch,
          lastEpochTo: done.outcome.last_epoch_to,
          historyFrom: done.outcome.history_floor,
          snapshotBasis: done.outcome.snapshot_basis,
          snapshotRejected: done.outcome.snapshot_rejected,
          verified: done.verified,
          staleness: done.staleness,
          changed: done.outcome.epochs_applied > 0 || done.outcome.tail_changed,
          epochsApplied: done.outcome.epochs_applied,
          cold,
          timing,
          network: this.#summaryFrom(this.#records.slice(before), engine),
        };
        this.#lastFeed = feed;
        opts.onProgress?.(this.#progress('idle', 1, 1, before, t0));
        return feed;
      }

      if (step.step === 'rpc') {
        // Ring 6 parks exactly like the transport. With anchorPolicy 'off' the
        // honest answer is `unavailable` — a VALUE, never a throw (LIVE-6: a
        // capability gap must never read as corruption).
        const tp = nowMs();
        stepJson = engine.sync_supply_rpc(JSON.stringify({ seq: step.seq, unavailable: true }), null);
        phases.anchor += nowMs() - tp;
        continue;
      }

      const fetchStep: StepFetch = step;
      if (fetchStep.artifact === 'epoch') {
        epochsSeen += 1;
        epochsTotal = Math.max(epochsTotal, epochsSeen + fetchStep.prefetch.length);
      }
      opts.onProgress?.(
        this.#progress(phaseOf(fetchStep.artifact), epochsSeen, epochsTotal, before, t0),
      );

      const got = await this.#satisfy(storage, fetchStep, staged, phases, opts.signal);
      let payload: Uint8Array | null = null;
      if (fetchStep.compressed && got.compressed) {
        const cap = Math.min(fetchStep.decompress_cap ?? this.#opts.maxArtifactBytes, this.#opts.maxArtifactBytes);
        const td = nowMs();
        // Hash the COMPRESSED bytes first. For an epoch nothing in Rust ever
        // sees the `.zst`, so this is the only place that check can happen;
        // running it before the inflate is what makes it a defence rather than
        // a post-mortem.
        try {
          verifyServedHash(got.compressed, fetchStep.sha256, fetchStep.path);
        } catch (e) {
          // §4.5's invalidation table is honoured for the folded blob and was
          // not for `artifacts`: a row that fails its hash was left in place,
          // so one bad read became a permanent sync failure until somebody
          // called `resetCache()`. Bytes that came off the wire are a feed
          // defect and must be seen; bytes that came out of IndexedDB are OUR
          // copy going bad, and the copy is what has to go.
          if (got.source === 'idb-cache') await storage.artifactDelete(artifactKey(fetchStep));
          throw e;
        }
        payload = inflateWithin(got.compressed, cap, fetchStep.path);
        phases.decompress += nowMs() - td;
      }

      const env: ResponseEnvelope = {
        seq: fetchStep.seq,
        status: got.status,
        not_modified: got.status === 304,
        absent: got.status === 404,
        etag: got.etag,
      };
      const ta = nowMs();
      stepJson = engine.sync_supply(JSON.stringify(env), got.compressed, payload);
      phases.apply += nowMs() - ta;

      // Design R: the bytes exactly as served are what gets persisted, so a
      // reload re-runs the same verification ladder over the same bytes the
      // network would have delivered.
      // The hash recorded is the one these bytes were VERIFIED against a few
      // lines above — never a hash nothing checked. A row that carries the
      // hash it was admitted under can be rejected cheaply on a later read.
      if (got.source === 'network' && got.compressed && cacheable(fetchStep.artifact)) {
        await storage.artifactPut(artifactKey(fetchStep), {
          hash: fetchStep.sha256 ?? '',
          zbytes: got.compressed,
        });
      }
    }
  }

  async #satisfy(
    storage: StorageAdapter,
    step: StepFetch,
    staged: Map<string, Staged>,
    phases: SyncTiming['phases'],
    signal: AbortSignal | undefined,
  ): Promise<{ compressed: Uint8Array | null; status: number; etag: string | null; source: RequestRecord['source'] }> {
    // 1. a prefetch we already have in hand
    const hit = staged.get(step.path);
    if (hit) {
      staged.delete(step.path);
      return { compressed: hit.compressed, status: hit.status, etag: hit.etag, source: hit.source };
    }

    // 2. IndexedDB, when the artifact is one we persist. An IDB hit is NOT a
    //    network request, and §9's table forbids counting it as one.
    if (this.#opts.persist !== 'folded' && cacheable(step.artifact)) {
      const t0 = nowMs();
      const stored = await storage.artifactGet(artifactKey(step));
      // A row whose recorded hash is not the one this step expects is not this
      // artifact — a stale epoch under a reused key, or a row written before
      // the hash was recorded. Cheaper to reject here than to inflate it and
      // find out, and rejecting is a cache miss, not an error.
      if (stored && step.sha256 && stored.hash !== step.sha256) {
        await storage.artifactDelete(artifactKey(step));
      } else if (stored) {
        cacheRecord(
          this.#net,
          { base: this.#opts.feedUrl, path: step.path, artifact: step.artifact, purpose: 'feed' },
          stored.zbytes.length,
          nowMs() - t0,
        );
        return { compressed: stored.zbytes, status: 200, etag: null, source: 'idb-cache' };
      }
    }

    // 3. the wire, with the module's own prefetch hints in flight beside it
    const tf = nowMs();
    const primary = request(this.#net, {
      base: this.#opts.feedUrl,
      path: step.path,
      artifact: step.artifact,
      purpose: 'feed',
      optional: step.optional,
      ifNoneMatch: step.conditional?.if_none_match ?? null,
      signal,
    });
    const hints = step.prefetch
      .filter((p) => !staged.has(p.path))
      .slice(0, Math.max(0, this.#opts.prefetchConcurrency - 1));
    const side = hints.map(async (p) => {
      try {
        const r = await request(this.#net, {
          base: this.#opts.feedUrl,
          path: p.path,
          artifact: p.artifact,
          purpose: 'feed',
          optional: false,
          signal,
        });
        if (r.bytes) staged.set(p.path, { compressed: r.bytes, status: r.status, etag: r.etag, source: 'network' });
      } catch {
        // A hint that fails is a wasted GET and nothing more; the module will
        // ask for the artifact again and that ask is the authority.
      }
    });
    const out = await primary;
    await Promise.all(side);
    phases.fetch += nowMs() - tf;

    // Prefetch bytes are NOT persisted here. They used to be, with `hash: ''`
    // — bytes nothing had hash-checked, written into the cache under the
    // manifest's key, which is a hostile feed's cheapest way to leave a
    // permanent artifact behind. A hint that gets consumed goes through the
    // ordinary path above and is persisted there, after `verifyServedHash`; a
    // hint that does not get consumed was never verified and has no business
    // outliving the tab.
    return { compressed: out.bytes, status: out.status, etag: out.etag, source: 'network' };
  }

  async #persistFolded(storage: StorageAdapter, engine: Engine): Promise<void> {
    const frameCount = engine.export_begin();
    const frames: Uint8Array[] = [];
    for (let i = 0; i < frameCount; i++) frames.push(engine.export_chunk(i));
    const info = JSON.parse(engine.info()) as EngineInfo;
    const meta: StateMeta = {
      frames: frames.length,
      len: frames.reduce((n, f) => n + f.length, 0),
      sha256: '',
      stamp: `${info.chain_id}|${info.pool}|${info.last_epoch}|${info.last_epoch_hash}`,
      engine_version: info.engine_version,
      profile_hash: '',
      written_at: Date.now(),
      source_manifest_hash: '',
    };
    await storage.statePut(meta, frames);
    engine.export_end();
  }

  // -------------------------------------------------------------- getNotes

  async getNotes(
    account: Account,
    opts: { signal?: AbortSignal; onProgress?: (p: Progress) => void; refresh?: 'auto' | 'force' | 'none' } = {},
  ): Promise<NotesResult> {
    return this.#serialize(async () => {
      const refresh = opts.refresh ?? 'auto';
      let feed = this.#lastFeed;
      if (refresh !== 'none' || !feed) {
        feed = await this.#syncOnce(opts);
      }
      return this.#discover(account, feed, opts.onProgress);
    });
  }

  async #discover(
    account: Account,
    feed: FeedState,
    onProgress?: (p: Progress) => void,
  ): Promise<NotesResult> {
    const storage = await this.#ensureOpen();
    const engine = await this.#ensureEngine();
    this.#accounts.add(account.address);

    let key: Uint8Array;
    try {
      key = await account.viewingKey();
    } catch {
      throw new Strk20Error('KEY_UNAVAILABLE', 'the account declined to supply a viewing key');
    }
    assertKey(key);

    const kid = deriveKeyId(key, this.#profile.chainId, this.#profile.pool, account.address);
    const sealed = await storage.cursorGet(kid);
    const entropy = randomBytes(32);

    const t0 = nowMs();
    let handle: number;
    try {
      // The module zeroizes the staging buffer; we zeroize again on every path
      // out, because a rejected call must not leave the bytes behind.
      handle = engine.discover_begin(account.address, key, sealed, entropy);
    } catch (e) {
      zeroize(key);
      zeroize(entropy);
      throw Strk20Error.fromModuleJson(e);
    }
    zeroize(key);

    try {
      // `stepBudgetMs` is a TYPESCRIPT budget. The module has no clock; we
      // calibrate ops-per-millisecond across calls and pick our own slice.
      let opsPerMs = 200;
      for (;;) {
        const budget = Math.max(1, Math.round(opsPerMs * this.#opts.stepBudgetMs));
        const t = nowMs();
        const out = JSON.parse(engine.discover_step(handle, budget)) as DiscoverStepOut;
        const dt = Math.max(0.01, nowMs() - t);
        opsPerMs = out.ops > 0 ? out.ops / dt : opsPerMs;
        onProgress?.(this.#progress('discover', out.ops_total, out.ops_total, this.#records.length, t0));
        if (out.done) break;
      }
      const result = engine.discover_finish(handle);
      const elapsedMs = nowMs() - t0;
      await storage.cursorPut(kid, result.sealed);

      const report = JSON.parse(result.report_json) as { notes: RawNote[] };
      const stats = JSON.parse(result.stats_json) as {
        slots_read: number;
        events_scanned: number;
        passes_in: number;
        passes_out: number;
        cursor_reset: boolean;
      };
      const notes = report.notes.map(toNote);
      const balances = new Map<string, bigint>();
      for (const n of notes) {
        if (n.spent) continue;
        balances.set(n.token, (balances.get(n.token) ?? 0n) + n.amount);
      }
      return {
        notes,
        balances,
        added: (JSON.parse(result.added_json) as RawNote[]).map(toNote),
        spent: (JSON.parse(result.spent_json) as RawNote[]).map(toNote),
        feed,
        complete: true,
        historyFrom: feed.historyFrom,
        cursorReset: stats.cursor_reset,
        stats: {
          slotsRead: stats.slots_read,
          eventsScanned: stats.events_scanned,
          passesIn: stats.passes_in,
          passesOut: stats.passes_out,
        },
        elapsedMs,
        raw: JSON.parse(result.report_json),
      };
    } catch (e) {
      engine.discover_abort(handle);
      throw Strk20Error.fromModuleJson(e);
    } finally {
      zeroize(entropy);
    }
  }

  // ----------------------------------------------------------------- watch

  watch(account: Account, cb: (ev: DiscoveryEvent) => void): Subscription {
    let closed = false;
    let live: LiveStream | null = null;
    let timer: ReturnType<typeof setTimeout> | null = null;

    const pass = async (): Promise<void> => {
      if (closed || this.#closed) return;
      try {
        const res = await this.getNotes(account, { refresh: 'auto' });
        cb({ type: 'feed', feed: res.feed });
        cb({
          type: 'notes',
          added: res.added,
          spent: res.spent,
          balances: res.balances,
          head: res.feed.head,
          elapsedMs: res.elapsedMs,
        });
      } catch (e) {
        const err = e instanceof Strk20Error ? e : Strk20Error.fromModuleJson(e);
        if (err.code === 'KEY_UNAVAILABLE') {
          cb({ type: 'status', state: 'locked' });
          return;
        }
        cb({ type: 'error', error: err, recovering: err.retryable });
      }
    };

    const startPolling = (): void => {
      this.#transport = 'polling';
      cb({ type: 'status', state: 'polling' });
      const tick = (): void => {
        if (closed) return;
        void pass().finally(() => {
          if (!closed) timer = setTimeout(tick, this.#opts.pollIntervalMs);
        });
      };
      timer = setTimeout(tick, this.#opts.pollIntervalMs);
    };

    if (this.#opts.live) {
      live = openLive(this.#net, this.#opts.feedUrl, {
        onPoke: () => {
          this.#transport = 'sse';
          cb({ type: 'status', state: 'live' });
          void pass();
        },
        onError: () => {
          // A static-file mirror publishes no stream, and that is a fully
          // supported deployment (§2.5). Degrading to polling is normal, not a
          // failure — but it is SAID, never hidden.
          live?.close();
          live = null;
          cb({ type: 'status', state: 'degraded' });
          startPolling();
        },
      });
    } else {
      startPolling();
    }

    void pass();

    return {
      close(): void {
        closed = true;
        live?.close();
        if (timer) clearTimeout(timer);
      },
      get closed() {
        return closed;
      },
    };
  }

  // --------------------------------------------------------------- history

  async history(
    account: Account,
    opts: { fromBlock?: number; limit?: number; signal?: AbortSignal } = {},
  ): Promise<{ transactions: HistoryTx[]; complete: boolean; completeFrom: number; registrationAvailable: boolean }> {
    return this.#serialize(async () => {
      const engine = await this.#ensureEngine();
      const storage = await this.#ensureOpen();
      const key = await account.viewingKey();
      assertKey(key);
      const kid = deriveKeyId(key, this.#profile.chainId, this.#profile.pool, account.address);
      const sealed = await storage.cursorGet(kid);
      try {
        const raw = JSON.parse(
          engine.history(account.address, key, sealed, opts.fromBlock ?? null, opts.limit ?? 100),
        ) as {
          transactions: (Omit<HistoryTx, 'amount'> & { amount: string })[];
          complete: boolean;
          complete_from: number;
          registration_available: boolean;
        };
        return {
          transactions: raw.transactions.map((t) => ({ ...t, amount: BigInt(t.amount) })),
          complete: raw.complete,
          completeFrom: raw.complete_from,
          registrationAvailable: raw.registration_available,
        };
      } finally {
        zeroize(key);
      }
    });
  }

  provider(account: Account): DiscoveryProvider {
    const client = this;
    return {
      async getIncomingNotes() {
        const r = await client.getNotes(account);
        return { notes: r.notes.filter((n) => !n.spent), cursor: null, complete: r.complete };
      },
      async getOutgoingNotes() {
        const r = await client.getNotes(account);
        return { notes: r.notes.filter((n) => n.spent), cursor: null, complete: r.complete };
      },
      async getTransactionHistory(o) {
        return client.history(account, o);
      },
    };
  }

  // ---------------------------------------------------------------- status

  status(): ClientStatus {
    const feed = this.#lastFeed;
    return {
      mode: 'keyless',
      transport: this.#transport,
      persistence: this.#storage?.kind ?? 'memory',
      persisted: this.#persisted,
      persistMode: this.#opts.persist,
      blocking: true,
      leader: true,
      engineBytes: this.#engine?.memoryBytes() ?? 0,
      head: feed?.head ?? 0,
      l1Accepted: feed?.l1Accepted ?? 0,
      lastEpoch: feed?.lastEpoch ?? -1,
      historyFrom: feed?.historyFrom ?? 0,
      verified: feed?.verified ?? 'replayed',
      accounts: this.#accounts.size,
      network: { requests: this.#countNetwork(), bytes: this.#records.reduce((n, r) => n + r.bytes, 0) },
      engine: {
        kind: this.#engineFactory.kind,
        label: this.#engineFactory.label,
        provenance: this.#engineFactory.provenance,
      },
    };
  }

  network(): { records: readonly RequestRecord[]; summary: NetworkSummary } {
    return { records: this.#records, summary: this.#summaryFrom(this.#records, this.#engine) };
  }

  /** Wasm instantiation / engine construction time, or null when not measured. */
  bootMs(): number | null {
    return this.#boot?.engineCreatedMs ?? null;
  }

  async resetCache(opts: { identities?: boolean } = {}): Promise<void> {
    const storage = await this.#ensureOpen();
    await storage.artifactClear();
    await storage.stateClear();
    if (opts.identities) await storage.cursorClear();
    this.#lastFeed = null;
    this.#fromCache = 'none';
  }

  async close(): Promise<void> {
    this.#closed = true;
    this.#engine?.free();
    this.#engine = null;
    this.#storage?.close();
    this.#storage = null;
  }

  /** §4 Stage 1's cold-start guard. Deleting the database is the caller's move. */
  async deleteDatabase(): Promise<'deleted' | 'blocked' | 'unavailable'> {
    await this.close();
    return deleteDatabase(this.databaseName);
  }

  // ------------------------------------------------------------- internals

  #countNetwork(): number {
    return this.#records.filter((r) => r.source !== 'idb-cache').length;
  }

  #summaryFrom(records: readonly RequestRecord[], engine: Engine | null): NetworkSummary {
    const byArtifact: Record<string, { requests: number; bytes: number }> = {};
    let bytes = 0;
    for (const r of records) {
      const slot = (byArtifact[r.artifact] ??= { requests: 0, bytes: 0 });
      slot.requests += 1;
      slot.bytes += r.bytes;
      bytes += r.bytes;
    }
    return {
      requests: records.length,
      bytes,
      byArtifact,
      // The hash comes from the module, not from this list. That is the
      // difference between "our UI says the lists match" and "the component
      // that authors the URLs cannot see a key, and here is its own hash".
      requestLogSha256: engine ? engine.request_log_sha256() : '',
    };
  }

  #progress(phase: Phase, done: number, total: number, since: number, t0: number): Progress {
    const slice = this.#records.slice(since);
    return {
      phase,
      done,
      total: Math.max(total, done),
      bytes: slice.reduce((n, r) => n + r.bytes, 0),
      requests: slice.filter((r) => r.source !== 'idb-cache').length,
      elapsedMs: nowMs() - t0,
    };
  }

  #serialize<T>(f: () => Promise<T>): Promise<T> {
    // All engine access is serialized inside the client: the wasm Engine is
    // `&mut` for both sync and discovery, so there is no concurrency to be had.
    const next = this.#busy.then(f, f);
    this.#busy = next.then(
      () => undefined,
      () => undefined,
    );
    return next;
  }
}

// ------------------------------------------------------------------ helpers

/**
 * A note as it appears in `report_json`.
 *
 * Both spellings are accepted, because `report_json` is CANONICAL and the two
 * engines canonicalise differently: the wasm module returns
 * `strk20_consumer::sync::SyncReport` verbatim — field-identical to
 * `strk20-sync sync --json`, which is snake_case — while the mock speaks the
 * package's own camelCase. Rewriting the module's report to match the wrapper
 * would break the one property the wasm build is worth having: that its report
 * is byte-identical to the native client's.
 */
interface RawNote {
  token: string;
  index: number;
  noteId?: string;
  note_id?: string;
  nullifier: string;
  amount: string;
  blockNumber?: number;
  block_number?: number;
  blockTimestamp?: number;
  block_timestamp?: number;
  sender: string;
  spent: boolean;
}

function toNote(r: RawNote): Note {
  return {
    token: r.token,
    index: r.index,
    noteId: r.noteId ?? r.note_id ?? '',
    nullifier: r.nullifier,
    amount: BigInt(r.amount),
    blockNumber: r.blockNumber ?? r.block_number ?? 0,
    blockTimestamp: r.blockTimestamp ?? r.block_timestamp ?? 0,
    sender: r.sender,
    spent: r.spent,
  };
}

function phaseOf(a: StepFetch['artifact']): Phase {
  switch (a) {
    case 'genesis':
      return 'open';
    case 'manifest':
      return 'manifest';
    case 'snapshot':
    case 'snapshot_anchor':
      return 'snapshot';
    case 'head':
      return 'head';
    case 'anchors':
    case 'epoch_anchor':
      return 'anchor';
    default:
      return 'epochs';
  }
}

function cacheable(a: StepFetch['artifact']): boolean {
  // §4.4: never stored — head.ndjson bytes, the head ETag, anything
  // tail-derived. The schema has nowhere to put a tail.
  return a === 'epoch' || a === 'snapshot' || a === 'snapshot_anchor' || a === 'epoch_anchor' || a === 'anchors';
}

function artifactKey(step: StepFetch): string {
  return pathKey(step.path);
}

function pathKey(path: string): string {
  const m = /\/epochs\/([0-9]{8})\./.exec(path);
  if (m) return `epoch:${Number(m[1])}`;
  if (path.startsWith('/snapshot')) return path.includes('anchor') ? 'anchor' : 'snapshot';
  return path;
}

function verifyServedHash(compressed: Uint8Array, expected: string | null, path: string): void {
  if (!expected) return;
  const actual = sha256Hex(compressed);
  if (actual !== expected) {
    throw new Strk20Error('FEED_HASH_MISMATCH', 'the served bytes do not match the hash the manifest published', {
      artifact: path,
      expected,
      actual,
    });
  }
}

export function inflateWithin(compressed: Uint8Array, cap: number, path: string): Uint8Array {
  // TypeScript's SOLE obligation here is the cap (§4.7). The module hashes both
  // buffers, so a decoder bug or a substituted payload becomes a loud hash
  // mismatch inside the engine — this is a resource bound, not a verification.
  //
  // It has to bound what gets ALLOCATED. The one-shot `decompress()` reads the
  // frame header's declared content size and allocates that up front, so a
  // `if (out.length > cap)` after the call is a post-mortem: a frame declaring
  // a terabyte has already asked for a terabyte by the time the comparison
  // runs, and a hostile feed only has to publish a manifest whose `zst` hash
  // matches the bomb (the hash check upstream passes, because the bomb IS the
  // bytes it published). The streaming decoder allocates only the frame's
  // window and hands output over block by block, so the cap is enforced while
  // inflating and the run stops at the first block that crosses it.
  const chunks: Uint8Array[] = [];
  let total = 0;
  const d = new Decompress((chunk) => {
    total += chunk.length;
    if (total > cap) {
      throw new Strk20Error('DECOMPRESS_LIMIT', 'inflated artifact exceeds its declared cap', {
        artifact: path,
        cap,
        got: total,
      });
    }
    if (chunk.length) chunks.push(chunk);
  });
  d.push(compressed, true);
  if (chunks.length === 1) return chunks[0]!;
  const out = new Uint8Array(total);
  let off = 0;
  for (const c of chunks) {
    out.set(c, off);
    off += c.length;
  }
  return out;
}

function randomBytes(n: number): Uint8Array {
  const b = new Uint8Array(n);
  const c = (globalThis as { crypto?: Crypto }).crypto;
  if (!c?.getRandomValues) {
    throw new Strk20Error('ENTROPY_INVALID', 'crypto.getRandomValues is unavailable');
  }
  c.getRandomValues(b);
  return b;
}

function nowMs(): number {
  const p = (globalThis as { performance?: Performance }).performance;
  return p?.now ? p.now() : Date.now();
}

function cfg(option: string, message: string, extra: Record<string, string | number | boolean | null> = {}): Strk20Error {
  return new Strk20Error('CONFIG_INVALID', message, { option, ...extra });
}
