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
import { decompress } from 'fzstd';
import { assertKey, zeroize } from "./account.js";
import { Strk20Error } from "./errors.js";
import { keyId as deriveKeyId } from "./kdf.js";
import { cacheRecord, openLive, request, resolveFetch, } from "./net.js";
import { resolveProfile } from "./profiles.js";
import { sha256Hex } from "./sha256.js";
import { dbName, deleteDatabase, openStorage } from "./storage.js";
const ZERO_PHASES = {
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
export class KeylessClient {
    #opts;
    #profile;
    #engineFactory;
    #persistence;
    #requestPersistentStorage;
    #dbSuffix;
    #onRequest;
    #net;
    #storage = null;
    #engine = null;
    #records = [];
    #busy = Promise.resolve();
    #closed = false;
    #persisted = false;
    #fromCache = 'none';
    #lastFeed = null;
    #accounts = new Set();
    #transport = 'polling';
    #openMs = 0;
    #loadMs = 0;
    #boot = null;
    constructor(opts) {
        if (!opts.feedUrl)
            throw cfg('feedUrl', 'a feed URL is required');
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
        if (!opts.engine)
            throw cfg('engine', 'an engine factory must be bound explicitly');
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
    get databaseName() {
        return dbName(this.#profile.chainId, this.#profile.pool) + this.#dbSuffix;
    }
    get profile() {
        return this.#profile;
    }
    // ------------------------------------------------------------------ sync
    async sync(opts = {}) {
        return this.#serialize(() => this.#syncOnce(opts));
    }
    async #ensureOpen() {
        if (this.#storage)
            return this.#storage;
        const t0 = nowMs();
        this.#storage = await openStorage(this.#persistence, this.databaseName);
        if (this.#requestPersistentStorage) {
            const s = globalThis.navigator?.storage;
            try {
                this.#persisted = (await s?.persist?.()) ?? false;
            }
            catch {
                this.#persisted = false;
            }
        }
        else {
            try {
                const s = globalThis.navigator?.storage;
                this.#persisted = (await s?.persisted?.()) ?? false;
            }
            catch {
                this.#persisted = false;
            }
        }
        this.#openMs = nowMs() - t0;
        return this.#storage;
    }
    async #ensureEngine() {
        if (this.#engine)
            return this.#engine;
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
    async #syncOnce(opts) {
        const t0 = nowMs();
        const storage = await this.#ensureOpen();
        const engine = await this.#ensureEngine();
        const phases = { ...ZERO_PHASES, open: this.#openMs, load: this.#loadMs };
        const before = this.#records.length;
        const cold = JSON.parse(engine.info()).last_epoch < 0;
        const staged = new Map();
        let stepJson = engine.sync_begin(this.#opts.coldStart);
        let epochsSeen = 0;
        let epochsTotal = 0;
        for (;;) {
            if (opts.signal?.aborted) {
                engine.sync_abort();
                throw new Strk20Error('ABORTED', 'sync aborted');
            }
            const step = JSON.parse(stepJson);
            if (step.step === 'done') {
                const done = step;
                if (done.state_dirty && this.#opts.persist !== 'raw') {
                    const tp = nowMs();
                    await this.#persistFolded(storage, engine);
                    phases.export += nowMs() - tp;
                }
                const timing = {
                    totalMs: nowMs() - t0,
                    phases,
                    cold,
                    fromCache: this.#fromCache,
                };
                const info = JSON.parse(engine.info());
                const feed = {
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
            const fetchStep = step;
            if (fetchStep.artifact === 'epoch') {
                epochsSeen += 1;
                epochsTotal = Math.max(epochsTotal, epochsSeen + fetchStep.prefetch.length);
            }
            opts.onProgress?.(this.#progress(phaseOf(fetchStep.artifact), epochsSeen, epochsTotal, before, t0));
            const got = await this.#satisfy(storage, fetchStep, staged, phases, opts.signal);
            let payload = null;
            if (fetchStep.compressed && got.compressed) {
                const cap = Math.min(fetchStep.decompress_cap ?? this.#opts.maxArtifactBytes, this.#opts.maxArtifactBytes);
                const td = nowMs();
                // Hash the COMPRESSED bytes first. For an epoch nothing in Rust ever
                // sees the `.zst`, so this is the only place that check can happen;
                // running it before the inflate is what makes it a defence rather than
                // a post-mortem.
                verifyServedHash(got.compressed, fetchStep.sha256, fetchStep.path);
                payload = inflateWithin(got.compressed, cap, fetchStep.path);
                phases.decompress += nowMs() - td;
            }
            const env = {
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
            if (got.source === 'network' && got.compressed && cacheable(fetchStep.artifact)) {
                await storage.artifactPut(artifactKey(fetchStep), { hash: '', zbytes: got.compressed });
            }
        }
    }
    async #satisfy(storage, step, staged, phases, signal) {
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
            if (stored) {
                cacheRecord(this.#net, { base: this.#opts.feedUrl, path: step.path, artifact: step.artifact, purpose: 'feed' }, stored.zbytes.length, nowMs() - t0);
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
                if (r.bytes)
                    staged.set(p.path, { compressed: r.bytes, status: r.status, etag: r.etag, source: 'network' });
            }
            catch {
                // A hint that fails is a wasted GET and nothing more; the module will
                // ask for the artifact again and that ask is the authority.
            }
        });
        const out = await primary;
        await Promise.all(side);
        phases.fetch += nowMs() - tf;
        if (out.bytes && cacheable(step.artifact)) {
            for (const [path, s] of staged) {
                if (s.source === 'network')
                    await storage.artifactPut(pathKey(path), { hash: '', zbytes: s.compressed });
            }
        }
        return { compressed: out.bytes, status: out.status, etag: out.etag, source: 'network' };
    }
    async #persistFolded(storage, engine) {
        const frameCount = engine.export_begin();
        const frames = [];
        for (let i = 0; i < frameCount; i++)
            frames.push(engine.export_chunk(i));
        const info = JSON.parse(engine.info());
        const meta = {
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
    async getNotes(account, opts = {}) {
        return this.#serialize(async () => {
            const refresh = opts.refresh ?? 'auto';
            let feed = this.#lastFeed;
            if (refresh !== 'none' || !feed) {
                feed = await this.#syncOnce(opts);
            }
            return this.#discover(account, feed, opts.onProgress);
        });
    }
    async #discover(account, feed, onProgress) {
        const storage = await this.#ensureOpen();
        const engine = await this.#ensureEngine();
        this.#accounts.add(account.address);
        let key;
        try {
            key = await account.viewingKey();
        }
        catch {
            throw new Strk20Error('KEY_UNAVAILABLE', 'the account declined to supply a viewing key');
        }
        assertKey(key);
        const kid = deriveKeyId(key, this.#profile.chainId, this.#profile.pool, account.address);
        const sealed = await storage.cursorGet(kid);
        const entropy = randomBytes(32);
        const t0 = nowMs();
        let handle;
        try {
            // The module zeroizes the staging buffer; we zeroize again on every path
            // out, because a rejected call must not leave the bytes behind.
            handle = engine.discover_begin(account.address, key, sealed, entropy);
        }
        catch (e) {
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
                const out = JSON.parse(engine.discover_step(handle, budget));
                const dt = Math.max(0.01, nowMs() - t);
                opsPerMs = out.ops > 0 ? out.ops / dt : opsPerMs;
                onProgress?.(this.#progress('discover', out.ops_total, out.ops_total, this.#records.length, t0));
                if (out.done)
                    break;
            }
            const result = engine.discover_finish(handle);
            const elapsedMs = nowMs() - t0;
            await storage.cursorPut(kid, result.sealed);
            const report = JSON.parse(result.report_json);
            const stats = JSON.parse(result.stats_json);
            const notes = report.notes.map(toNote);
            const balances = new Map();
            for (const n of notes) {
                if (n.spent)
                    continue;
                balances.set(n.token, (balances.get(n.token) ?? 0n) + n.amount);
            }
            return {
                notes,
                balances,
                added: JSON.parse(result.added_json).map(toNote),
                spent: JSON.parse(result.spent_json).map(toNote),
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
        }
        catch (e) {
            engine.discover_abort(handle);
            throw Strk20Error.fromModuleJson(e);
        }
        finally {
            zeroize(entropy);
        }
    }
    // ----------------------------------------------------------------- watch
    watch(account, cb) {
        let closed = false;
        let live = null;
        let timer = null;
        const pass = async () => {
            if (closed || this.#closed)
                return;
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
            }
            catch (e) {
                const err = e instanceof Strk20Error ? e : Strk20Error.fromModuleJson(e);
                if (err.code === 'KEY_UNAVAILABLE') {
                    cb({ type: 'status', state: 'locked' });
                    return;
                }
                cb({ type: 'error', error: err, recovering: err.retryable });
            }
        };
        const startPolling = () => {
            this.#transport = 'polling';
            cb({ type: 'status', state: 'polling' });
            const tick = () => {
                if (closed)
                    return;
                void pass().finally(() => {
                    if (!closed)
                        timer = setTimeout(tick, this.#opts.pollIntervalMs);
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
        }
        else {
            startPolling();
        }
        void pass();
        return {
            close() {
                closed = true;
                live?.close();
                if (timer)
                    clearTimeout(timer);
            },
            get closed() {
                return closed;
            },
        };
    }
    // --------------------------------------------------------------- history
    async history(account, opts = {}) {
        return this.#serialize(async () => {
            const engine = await this.#ensureEngine();
            const storage = await this.#ensureOpen();
            const key = await account.viewingKey();
            assertKey(key);
            const kid = deriveKeyId(key, this.#profile.chainId, this.#profile.pool, account.address);
            const sealed = await storage.cursorGet(kid);
            try {
                const raw = JSON.parse(engine.history(account.address, key, sealed, opts.fromBlock ?? null, opts.limit ?? 100));
                return {
                    transactions: raw.transactions.map((t) => ({ ...t, amount: BigInt(t.amount) })),
                    complete: raw.complete,
                    completeFrom: raw.complete_from,
                    registrationAvailable: raw.registration_available,
                };
            }
            finally {
                zeroize(key);
            }
        });
    }
    provider(account) {
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
    status() {
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
    network() {
        return { records: this.#records, summary: this.#summaryFrom(this.#records, this.#engine) };
    }
    /** Wasm instantiation / engine construction time, or null when not measured. */
    bootMs() {
        return this.#boot?.engineCreatedMs ?? null;
    }
    async resetCache(opts = {}) {
        const storage = await this.#ensureOpen();
        await storage.artifactClear();
        await storage.stateClear();
        if (opts.identities)
            await storage.cursorClear();
        this.#lastFeed = null;
        this.#fromCache = 'none';
    }
    async close() {
        this.#closed = true;
        this.#engine?.free();
        this.#engine = null;
        this.#storage?.close();
        this.#storage = null;
    }
    /** §4 Stage 1's cold-start guard. Deleting the database is the caller's move. */
    async deleteDatabase() {
        await this.close();
        return deleteDatabase(this.databaseName);
    }
    // ------------------------------------------------------------- internals
    #countNetwork() {
        return this.#records.filter((r) => r.source !== 'idb-cache').length;
    }
    #summaryFrom(records, engine) {
        const byArtifact = {};
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
    #progress(phase, done, total, since, t0) {
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
    #serialize(f) {
        // All engine access is serialized inside the client: the wasm Engine is
        // `&mut` for both sync and discovery, so there is no concurrency to be had.
        const next = this.#busy.then(f, f);
        this.#busy = next.then(() => undefined, () => undefined);
        return next;
    }
}
function toNote(r) {
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
function phaseOf(a) {
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
function cacheable(a) {
    // §4.4: never stored — head.ndjson bytes, the head ETag, anything
    // tail-derived. The schema has nowhere to put a tail.
    return a === 'epoch' || a === 'snapshot' || a === 'snapshot_anchor' || a === 'epoch_anchor' || a === 'anchors';
}
function artifactKey(step) {
    return pathKey(step.path);
}
function pathKey(path) {
    const m = /\/epochs\/([0-9]{8})\./.exec(path);
    if (m)
        return `epoch:${Number(m[1])}`;
    if (path.startsWith('/snapshot'))
        return path.includes('anchor') ? 'anchor' : 'snapshot';
    return path;
}
function verifyServedHash(compressed, expected, path) {
    if (!expected)
        return;
    const actual = sha256Hex(compressed);
    if (actual !== expected) {
        throw new Strk20Error('FEED_HASH_MISMATCH', 'the served bytes do not match the hash the manifest published', {
            artifact: path,
            expected,
            actual,
        });
    }
}
function inflateWithin(compressed, cap, path) {
    // TypeScript's SOLE obligation here is the cap (§4.7). The module hashes both
    // buffers, so a decoder bug or a substituted payload becomes a loud hash
    // mismatch inside the engine — this is a resource bound, not a verification.
    const out = decompress(compressed);
    if (out.length > cap) {
        throw new Strk20Error('DECOMPRESS_LIMIT', 'inflated artifact exceeds its declared cap', {
            artifact: path,
            cap,
            got: out.length,
        });
    }
    return out;
}
function randomBytes(n) {
    const b = new Uint8Array(n);
    const c = globalThis.crypto;
    if (!c?.getRandomValues) {
        throw new Strk20Error('ENTROPY_INVALID', 'crypto.getRandomValues is unavailable');
    }
    c.getRandomValues(b);
    return b;
}
function nowMs() {
    const p = globalThis.performance;
    return p?.now ? p.now() : Date.now();
}
function cfg(option, message, extra = {}) {
    return new Strk20Error('CONFIG_INVALID', message, { option, ...extra });
}
//# sourceMappingURL=client.js.map