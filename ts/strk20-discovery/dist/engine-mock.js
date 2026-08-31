/**
 * The MOCK engine.
 *
 * It exists because the real computer is not built to §3.3's second-pass ABI
 * yet (see engine-wasm.ts for the exact delta). Rather than block the demo —
 * whose entire purpose is to surface API problems early — the engine sits
 * behind `EngineFactory` and this implementation satisfies the same ABI in
 * TypeScript.
 *
 * It parses the REAL feed schema: `data/mainnet/feed` in this repo can be served
 * as a static directory and this engine folds it. That is deliberate. The
 * network layer, the trampoline, the verification ladder and the persistence
 * are then exercised against real bytes, and the only thing that changes when
 * the wasm module lands is which factory the demo binds.
 *
 * WHAT IS REAL HERE, because a mock that lies is worse than no demo:
 *   - the Step trampoline is the real protocol: the same asks, in the same
 *     order, with the same envelopes;
 *   - the fetches are real HTTP against a real static feed directory;
 *   - the sha256 of BOTH the compressed and the inflated buffer is verified
 *     against the manifest, and the epoch hash chain (`hdr.prev`) is walked. A
 *     tampered epoch really does raise FEED_HASH_MISMATCH;
 *   - the fold is a real parse of the real NDJSON and a real application of
 *     every storage diff into a slot map — on mainnet that is 139,131 writes
 *     over 134,879 distinct slots, which is the cost the browser actually pays;
 *   - the request log and its sha256 are computed HERE, inside the key-blind
 *     component, exactly as §3.3 requires — not by the UI.
 *
 * WHAT IS NOT REAL, stated so no number is misread:
 *   - it cannot decrypt real notes. Discovery is a trial scan over `n` records,
 *     a mock-only note encoding that the replay fixture carries and a real feed
 *     does not. Against a real feed discovery honestly finds nothing and says
 *     why (`mock_notes_available: false` in the report);
 *   - the tag derivation is sha256, not the pool's note encryption, so the
 *     per-note cost is far below the real trial decryption;
 *   - there is no MPT and no snapshot lane, so `verified` never rises above
 *     `replayed`.
 *
 * Consequence, republished through `status().engine` and rendered in the demo's
 * badge: **no timing from this engine is comparable to a native or wasm
 * measurement.** It is comparable to itself — which is exactly enough for the
 * cold-versus-warm contrast, the request and byte counts, and the
 * identical-stream claim, which are what the demo is for.
 */
import { Strk20Error } from "./errors.js";
import { concatBytes, sha256, sha256Hex, toHex } from "./sha256.js";
const ENGINE_VERSION = 'mock-0.1.0';
const EPOCH_CAP = 64 * 1024 * 1024;
const MAX_FRAME = 4 * 1024 * 1024;
// Advisory hint depth. The trampoline satisfies one request at a time, and on
// the epochs lane that is 606 strictly sequential round trips — tens of seconds
// of pure latency on top of the fold. The window is what lets the wrapper put
// several in flight; it is a hint, never a contract, and nothing is ever
// applied because it was hinted.
const PREFETCH_WINDOW = 16;
const enc = new TextEncoder();
const dec = new TextDecoder();
function emptyMirror(p) {
    return {
        chain_id: p.chainId,
        pool: p.pool,
        genesis_block: p.genesisBlock,
        epoch_size: p.epochSize,
        last_epoch: -1,
        last_epoch_hash: '',
        last_epoch_to: p.genesisBlock - 1,
        history_floor: p.genesisBlock,
        head: 0,
        l1_accepted: 0,
        slots: Object.create(null),
        slotWrites: 0,
        notes: [],
        nullifiers: [],
        blocks: 0,
        events: 0,
        mockNotesSeen: false,
        tailNotes: [],
        tailNullifiers: [],
    };
}
// ------------------------------------------------------------- key derivation
// The mock's stand-in for note encryption. Structurally faithful: the tag is
// computable only with the viewing key, and the nullifier is PREDICTED by the
// owner and then looked for in the mirror — which is exactly the property the
// live run confirmed on real data (live-run §7).
export function noteTag(key, noteId) {
    return sha256Hex(concatBytes(enc.encode('strk20-mock-tag/'), key, enc.encode(noteId))).slice(0, 16);
}
export function noteNullifier(key, noteId) {
    return '0x' + sha256Hex(concatBytes(enc.encode('strk20-mock-nf/'), key, enc.encode(noteId))).slice(0, 62);
}
function keyStream(key, nonce, len) {
    const out = new Uint8Array(len);
    let o = 0;
    let c = 0;
    while (o < len) {
        const block = sha256(concatBytes(enc.encode('strk20-mock-seal/'), key, nonce, enc.encode(String(c++))));
        out.set(block.subarray(0, Math.min(32, len - o)), o);
        o += 32;
    }
    return out;
}
export class MockEngine {
    #profile;
    #mirror;
    #run = null;
    #askLog = [];
    #sessions = new Map();
    #nextHandle = 1;
    #usedEntropy = new Set();
    #dirty = false;
    #exportStage = null;
    #manifestHash = '';
    #verified = 'replayed';
    constructor(profile, mirror) {
        this.#profile = profile;
        this.#mirror = mirror ?? emptyMirror(profile);
    }
    /** Not part of the ABI. The demo pins it for the A/B comparison (§7 rule 2). */
    manifestHash() {
        return this.#manifestHash;
    }
    // ---------------------------------------------------------------- info
    info() {
        const m = this.#mirror;
        const out = {
            chain_id: m.chain_id,
            pool: m.pool,
            genesis_block: m.genesis_block,
            epoch_size: m.epoch_size,
            last_epoch: m.last_epoch,
            last_epoch_hash: m.last_epoch_hash,
            last_epoch_to: m.last_epoch_to,
            history_floor: m.history_floor,
            snapshot_basis: null,
            snapshot_pending_grounding: false,
            head: m.head,
            l1_accepted: m.l1_accepted,
            slots: Object.keys(m.slots).length,
            blocks: m.blocks,
            events: m.events,
            verified: this.#verified,
            engine_version: ENGINE_VERSION,
            state_dirty: this.#dirty,
        };
        return JSON.stringify(out);
    }
    // ------------------------------------------------------------ feed sync
    sync_begin(_coldStart) {
        if (this.#run)
            throw err('SYNC_IN_PROGRESS', 'a sync run is already open');
        // `snapshot` and `auto` both land on the epochs lane here: this feed
        // publishes no snapshot (roadmap item 1). That is the `auto` fallback doing
        // exactly what it is for, and it is why the fallback cannot live in
        // TypeScript — the decision is taken after the manifest is read.
        this.#run = {
            seq: 0,
            outstanding: null,
            manifest: null,
            pending: [],
            epochsApplied: 0,
            tailChanged: false,
            dirty: false,
        };
        return JSON.stringify(this.#ask({ artifact: 'genesis', path: '/genesis.json', compressed: false }));
    }
    sync_supply(metaJson, compressed, payload) {
        const run = this.#run;
        if (!run || !run.outstanding)
            throw err('SYNC_PROTOCOL', 'no outstanding step');
        const env = JSON.parse(metaJson);
        if (env.seq !== run.outstanding.seq) {
            throw err('SYNC_PROTOCOL', 'response sequence does not match the outstanding step', {
                expected: run.outstanding.seq,
                got: env.seq,
            });
        }
        const step = run.outstanding;
        run.outstanding = null;
        switch (step.artifact) {
            case 'genesis':
                return JSON.stringify(this.#onGenesis(need(compressed, step)));
            case 'manifest':
                return JSON.stringify(this.#onManifest(run, need(compressed, step)));
            case 'epoch':
                return JSON.stringify(this.#onEpoch(run, step, need(compressed, step), payload));
            case 'head':
                return JSON.stringify(this.#onHead(run, env, compressed));
            default:
                throw err('SYNC_PROTOCOL', 'unexpected artifact', { artifact: step.artifact });
        }
    }
    sync_supply_rpc(_metaJson, _resultJson) {
        // Ring 6 is a Step::Rpc on the same trampoline in the real module. This
        // engine has no MPT, so it emits no rpc step and never claims `anchored`.
        throw err('SYNC_PROTOCOL', 'the mock engine emits no rpc steps');
    }
    sync_abort() {
        this.#run = null;
    }
    // ------------------------------------------------------------ request log
    request_log() {
        return this.#askLog.join('\n');
    }
    request_log_sha256() {
        return sha256Hex(enc.encode(this.request_log()));
    }
    // --------------------------------------------------------- persisted state
    export_begin() {
        // Tail-derived rows are dropped HERE, not filtered later: §4.4's schema has
        // nowhere to put a tail, and this is that rule expressed in the export.
        const persistable = { ...this.#mirror, tailNotes: [], tailNullifiers: [] };
        const header = {
            v: 2,
            engine_version: ENGINE_VERSION,
            profile_hash: profileHash(this.#profile),
            stamp: {
                chain_id: this.#mirror.chain_id,
                pool: this.#mirror.pool,
                last_epoch: this.#mirror.last_epoch,
                last_epoch_hash: this.#mirror.last_epoch_hash,
                last_epoch_to: this.#mirror.last_epoch_to,
                history_floor: this.#mirror.history_floor,
                verified: this.#verified,
            },
        };
        const body = enc.encode(JSON.stringify(persistable));
        const trailer = enc.encode(JSON.stringify({ sha256: sha256Hex(body), len: body.length }));
        const head = enc.encode(JSON.stringify(header));
        const buf = concatBytes(u32(head.length), head, u32(body.length), body, u32(trailer.length), trailer);
        this.#exportStage = buf;
        return Math.ceil(buf.length / MAX_FRAME);
    }
    export_chunk(i) {
        const buf = this.#exportStage;
        if (!buf)
            throw err('SESSION_INVALID', 'export_begin was not called');
        return buf.slice(i * MAX_FRAME, Math.min(buf.length, (i + 1) * MAX_FRAME));
    }
    export_end() {
        this.#exportStage = null;
        this.#dirty = false;
    }
    static loadFrames(profile, frames) {
        try {
            const buf = concatBytes(...frames.map((f) => Uint8Array.from(f)));
            let o = 0;
            const read = () => {
                const n = new DataView(buf.buffer, buf.byteOffset + o, 4).getUint32(0, false);
                o += 4;
                const s = buf.subarray(o, o + n);
                o += n;
                return s;
            };
            const header = JSON.parse(dec.decode(read()));
            if (header.v !== 2)
                return null;
            if (header.engine_version !== ENGINE_VERSION)
                return null;
            if (header.profile_hash !== profileHash(profile))
                return null;
            if (header.stamp.chain_id !== profile.chainId || header.stamp.pool !== profile.pool)
                return null;
            const body = read();
            const trailer = JSON.parse(dec.decode(read()));
            if (trailer.len !== body.length)
                return null;
            if (sha256Hex(body) !== trailer.sha256)
                return null;
            const mirror = JSON.parse(dec.decode(body));
            mirror.tailNotes = [];
            mirror.tailNullifiers = [];
            return new MockEngine(profile, mirror);
        }
        catch {
            // Every failure here is a cache miss, never an error: deleting the folded
            // blob is always correct (§4.5 — Design M is strictly a cache).
            return null;
        }
    }
    // ------------------------------------------------------------- discovery
    discover_begin(ownerHex, key, sealed, entropy32) {
        if (key.length !== 32)
            throw err('KEY_INVALID', 'viewing key must be 32 bytes', { got: key.length });
        if (entropy32.length !== 32)
            throw err('ENTROPY_INVALID', 'entropy must be 32 bytes');
        const eHex = toHex(entropy32);
        if (this.#usedEntropy.has(eHex))
            throw err('ENTROPY_REUSED', 'entropy was already consumed');
        const held = Uint8Array.from(key);
        // "the caller's staging buffer is zeroized before return" — §3.3.
        key.fill(0);
        const prior = sealed ? openSeal(held, sealed) : null;
        const s = {
            handle: this.#nextHandle++,
            owner: ownerHex,
            key: held,
            entropy: Uint8Array.from(entropy32),
            view: [...this.#mirror.notes, ...this.#mirror.tailNotes],
            nullifiers: new Set([...this.#mirror.nullifiers, ...this.#mirror.tailNullifiers]),
            known: new Set(prior?.notes ?? []),
            knownSpent: new Set(prior?.spent ?? []),
            scanned: 0,
            found: [],
            spentNow: [],
            cursorReset: sealed != null && prior == null,
            done: false,
        };
        // A seal whose checkpoint is above our mirror indexes history we do not
        // have: treat as no cursor rather than trusting it (§4.5).
        if (prior && prior.ckpt_epoch > this.#mirror.last_epoch) {
            s.known.clear();
            s.knownSpent.clear();
            s.cursorReset = true;
        }
        this.#sessions.set(s.handle, s);
        return s.handle;
    }
    discover_step(handle, maxOps) {
        const s = this.#session(handle);
        const notes = s.view;
        const budget = Math.max(1, maxOps);
        let ops = 0;
        while (s.scanned < notes.length && ops < budget) {
            const row = notes[s.scanned];
            // The real trial decryption; here a tag derivation. O(1) per note, run
            // over the whole anonymity set — the cost shape that matters.
            if (noteTag(s.key, row.id) === row.tag)
                s.found.push(row);
            s.scanned++;
            ops++;
        }
        let phase = 'live_in';
        if (s.scanned >= notes.length) {
            // Spent-state pass: the owner PREDICTS each nullifier and looks for it.
            for (const n of s.found) {
                if (s.nullifiers.has(noteNullifier(s.key, n.id)) && !s.knownSpent.has(n.id))
                    s.spentNow.push(n.id);
            }
            s.done = true;
            phase = 'done';
        }
        const out = {
            done: s.done,
            phase,
            ops,
            ops_total: s.scanned,
            channels: 1,
            notes: s.found.length,
        };
        return JSON.stringify(out);
    }
    discover_finish(handle) {
        const s = this.#session(handle);
        if (!s.done)
            throw err('SESSION_INCOMPLETE', 'discover_step has not reported done');
        const spentIds = new Set([...s.knownSpent, ...s.spentNow]);
        const notes = s.found.map((n) => this.#toNote(n, s.key, spentIds.has(n.id)));
        const added = notes.filter((n) => !s.known.has(n.noteId));
        const spent = notes.filter((n) => s.spentNow.includes(n.noteId));
        const sealed = makeSeal(s.key, s.entropy, {
            v: 1,
            ckpt_epoch: this.#mirror.last_epoch,
            ckpt_epoch_hash: this.#mirror.last_epoch_hash,
            notes: notes.map((n) => n.noteId),
            spent: [...spentIds],
        });
        this.#usedEntropy.add(toHex(s.entropy));
        const report = {
            chain_id: this.#mirror.chain_id,
            pool: this.#mirror.pool,
            owner: s.owner,
            head: this.#mirror.head,
            last_epoch: this.#mirror.last_epoch,
            history_floor: this.#mirror.history_floor,
            complete: true,
            notes,
            engine: ENGINE_VERSION,
            /**
             * Mock-only, and the reason it exists: against a REAL feed there are no
             * `n` records to scan, so "0 notes" would otherwise be indistinguishable
             * from "this key owns nothing". The demo renders the difference.
             */
            mock_notes_available: this.#mirror.mockNotesSeen,
        };
        const stats = {
            slots_read: Object.keys(this.#mirror.slots).length,
            events_scanned: s.scanned,
            passes_in: 1,
            passes_out: 1,
            ops: s.scanned,
            cursor_reset: s.cursorReset,
        };
        s.key.fill(0);
        s.entropy.fill(0);
        this.#sessions.delete(handle);
        return {
            report_json: JSON.stringify(report),
            sealed,
            added_json: JSON.stringify(added),
            spent_json: JSON.stringify(spent),
            stats_json: JSON.stringify(stats),
        };
    }
    discover_abort(handle) {
        const s = this.#sessions.get(handle);
        if (!s)
            return;
        s.key.fill(0);
        s.entropy.fill(0);
        this.#sessions.delete(handle);
        // A session that never finishes never consumes its entropy (§3.3), so a
        // torn discovery cannot burn a nonce.
    }
    history(ownerHex, key, _sealed, fromBlock, limit) {
        const held = Uint8Array.from(key);
        key.fill(0);
        if (fromBlock != null && fromBlock < this.#mirror.history_floor) {
            held.fill(0);
            throw err('HISTORY_UNAVAILABLE', 'requested block is below the history floor', {
                from_block: fromBlock,
                history_floor: this.#mirror.history_floor,
            });
        }
        const nfs = new Set([...this.#mirror.nullifiers, ...this.#mirror.tailNullifiers]);
        const txs = [];
        for (const row of [...this.#mirror.notes, ...this.#mirror.tailNotes]) {
            if (noteTag(held, row.id) !== row.tag)
                continue;
            if (fromBlock != null && row.b < fromBlock)
                continue;
            txs.push({
                kind: 'deposit',
                blockNumber: row.b,
                blockTimestamp: row.t,
                token: row.tok,
                amount: row.amt,
                noteId: row.id,
                nullifier: null,
            });
            const nf = noteNullifier(held, row.id);
            if (nfs.has(nf)) {
                txs.push({
                    kind: 'withdraw',
                    blockNumber: row.b,
                    blockTimestamp: row.t,
                    token: row.tok,
                    amount: row.amt,
                    noteId: row.id,
                    nullifier: nf,
                });
            }
            if (txs.length >= limit)
                break;
        }
        held.fill(0);
        return JSON.stringify({
            owner: ownerHex,
            transactions: txs,
            complete: true,
            complete_from: this.#mirror.history_floor,
            registration_available: false,
        });
    }
    export_reference_cursor(key, sealed) {
        const held = Uint8Array.from(key);
        key.fill(0);
        const open = openSeal(held, sealed);
        held.fill(0);
        if (!open)
            throw err('SEALED_STATE_MISMATCH', 'sealed blob did not open under this key');
        return JSON.stringify({ version: 1, checkpoint: open.ckpt_epoch, notes: open.notes });
    }
    free() {
        for (const h of [...this.#sessions.keys()])
            this.discover_abort(h);
        this.#mirror = emptyMirror(this.#profile);
        this.#exportStage = null;
    }
    memoryBytes() {
        // A JS heap figure would be a guess, and a guess rendered next to measured
        // numbers is exactly what demo-app.md §9 rule 1 forbids. 0 means "this
        // engine cannot report it" and the UI prints `unavailable`.
        return 0;
    }
    // ------------------------------------------------------------- internals
    #session(handle) {
        const s = this.#sessions.get(handle);
        if (!s)
            throw err('SESSION_INVALID', 'no such discovery session', { handle });
        return s;
    }
    #toNote(row, key, spent) {
        return {
            token: row.tok,
            index: row.i,
            noteId: row.id,
            nullifier: noteNullifier(key, row.id),
            amount: row.amt,
            blockNumber: row.b,
            blockTimestamp: row.t,
            sender: row.from,
            spent,
        };
    }
    #ask(a) {
        const run = this.#run;
        run.seq += 1;
        // The published `.zst` hash, so the wrapper can check the served bytes
        // BEFORE inflating them. `#onEpoch` re-checks it afterwards; that one is
        // the authority, this one is the zip-bomb gate.
        const zstOf = (path) => run.manifest?.epochs.find((e) => e.e === epochIndexOf(path))?.zst ?? null;
        const step = {
            step: 'fetch',
            seq: run.seq,
            artifact: a.artifact,
            path: a.path,
            optional: false,
            compressed: a.compressed,
            decompress_cap: a.compressed ? EPOCH_CAP : null,
            sha256: a.artifact === 'epoch' ? zstOf(a.path) : null,
            conditional: a.conditional ? { if_none_match: a.conditional } : null,
            reason: reasonFor(a.artifact, a.path),
            prefetch: [],
        };
        if (a.artifact === 'epoch') {
            // Advisory only. Emitted from the same verified manifest the module is
            // already walking — no second planner, no second authority. Nothing is
            // ever applied because it was hinted.
            for (const e of run.pending.slice(0, PREFETCH_WINDOW)) {
                const path = epochPath(e);
                step.prefetch.push({
                    artifact: 'epoch',
                    path,
                    compressed: true,
                    decompress_cap: EPOCH_CAP,
                    sha256: zstOf(path),
                });
            }
        }
        run.outstanding = step;
        // The ask log is written HERE, by the key-blind component. That is the whole
        // point of §3.3 putting request_log inside the module rather than the UI.
        this.#askLog.push(JSON.stringify({ seq: step.seq, artifact: step.artifact, path: step.path }));
        return step;
    }
    #onGenesis(bytes) {
        const g = JSON.parse(dec.decode(bytes));
        // §3.10 item 3: genesis is checked against the profile the CALLER expects,
        // not only against stored meta. This is what closes trust-on-first-use — an
        // empty mirror must not adopt whatever chain the feed declares.
        if (g.chain_id !== this.#profile.chainId || g.pool !== this.#profile.pool) {
            this.#run = null;
            throw err('CHAIN_MISMATCH', 'feed genesis does not match the expected chain profile', {
                expected_chain_id: this.#profile.chainId,
                got_chain_id: g.chain_id,
                expected_pool: this.#profile.pool,
                got_pool: g.pool,
            });
        }
        if (g.genesis_block !== this.#profile.genesisBlock || g.epoch_size !== this.#profile.epochSize) {
            this.#run = null;
            throw err('CHAIN_MISMATCH', 'feed genesis disagrees with the profile geometry', {
                expected_genesis_block: this.#profile.genesisBlock,
                got_genesis_block: g.genesis_block,
                expected_epoch_size: this.#profile.epochSize,
                got_epoch_size: g.epoch_size,
            });
        }
        return this.#ask({ artifact: 'manifest', path: '/manifest.json', compressed: false });
    }
    #onManifest(run, bytes) {
        const m = JSON.parse(dec.decode(bytes));
        if (m.chain_id !== this.#profile.chainId || m.pool !== this.#profile.pool) {
            this.#run = null;
            throw err('CHAIN_MISMATCH', 'manifest chain identity differs from the profile');
        }
        if (!Array.isArray(m.epochs) || m.epochs.length === 0) {
            this.#run = null;
            throw err('FEED_MALFORMED', 'manifest declares no epochs');
        }
        run.manifest = m;
        this.#manifestHash = sha256Hex(bytes);
        // The manifest-divergence check (apply.rs:197-207): if the feed's hash for
        // an epoch we already applied differs, our history is not the feed's.
        if (this.#mirror.last_epoch >= 0 && this.#mirror.last_epoch_hash) {
            const entry = m.epochs.find((e) => e.e === this.#mirror.last_epoch);
            if (entry && entry.hash !== this.#mirror.last_epoch_hash) {
                this.#mirror = emptyMirror(this.#profile);
            }
        }
        this.#mirror.history_floor = m.epochs[0].from;
        run.pending = m.epochs.filter((e) => e.e > this.#mirror.last_epoch).map((e) => e.e);
        const next = run.pending.shift();
        if (next != null)
            return this.#ask({ artifact: 'epoch', path: epochPath(next), compressed: true });
        return this.#ask({ artifact: 'head', path: '/head.ndjson', compressed: false });
    }
    #onEpoch(run, step, compressed, payload) {
        if (!payload)
            throw err('DECOMPRESS_UNSTAGED', 'an epoch was supplied without its inflated payload');
        const idx = epochIndexOf(step.path);
        const entry = run.manifest.epochs.find((e) => e.e === idx);
        if (!entry)
            throw err('FEED_MALFORMED', 'epoch is absent from the manifest', { epoch: idx });
        // §3.10 item 1 and §3.4: BOTH buffers are hashed, and the compressed one is
        // checked FIRST, as the snapshot path already did. TypeScript performed no
        // verification whatsoever.
        const zHash = sha256Hex(compressed);
        if (zHash !== entry.zst) {
            throw err('FEED_HASH_MISMATCH', 'compressed epoch hash does not match the manifest', {
                epoch: idx,
                expected: entry.zst,
                got: zHash,
            });
        }
        const pHash = sha256Hex(payload);
        if (pHash !== entry.hash) {
            throw err('FEED_HASH_MISMATCH', 'epoch payload hash does not match the manifest', {
                epoch: idx,
                expected: entry.hash,
                got: pHash,
            });
        }
        const hdr = this.#fold(payload, 'base');
        // The hash chain is the trust story the feed has before anchors exist.
        if (this.#mirror.last_epoch >= 0 && hdr?.prev && hdr.prev !== this.#mirror.last_epoch_hash) {
            throw err('FEED_CHAIN_BROKEN', 'epoch header does not chain to the previously applied epoch', {
                epoch: idx,
                expected_prev: this.#mirror.last_epoch_hash,
                got_prev: hdr.prev,
            });
        }
        this.#mirror.last_epoch = idx;
        this.#mirror.last_epoch_hash = entry.hash;
        this.#mirror.last_epoch_to = entry.to;
        run.epochsApplied += 1;
        run.dirty = true;
        this.#dirty = true;
        const next = run.pending.shift();
        if (next != null)
            return this.#ask({ artifact: 'epoch', path: epochPath(next), compressed: true });
        return this.#ask({ artifact: 'head', path: '/head.ndjson', compressed: false });
    }
    #onHead(run, env, bytes) {
        const m = run.manifest;
        if (!env.not_modified && bytes) {
            // The tail is rebuilt from scratch every pass. §4.4: nothing tail-derived
            // is persisted, so there is no persisted reorg logic to get wrong.
            this.#mirror.tailNotes = [];
            this.#mirror.tailNullifiers = [];
            this.#fold(bytes, 'tail');
            run.tailChanged = true;
        }
        this.#mirror.head = m.head.number;
        this.#mirror.l1_accepted = m.head.l1_accepted;
        this.#run = null;
        const behind = m.head.number - this.#mirror.last_epoch_to;
        const staleness = behind > m.epoch_size * 2 ? 'behind' : 'ok';
        const done = {
            step: 'done',
            staleness,
            verified: this.#verified,
            state_dirty: run.dirty,
            outcome: {
                epochs_applied: run.epochsApplied,
                tail_rewound: false,
                tail_changed: run.tailChanged,
                head: this.#mirror.head,
                l1_accepted: this.#mirror.l1_accepted,
                last_epoch_to: this.#mirror.last_epoch_to,
                snapshot_basis: null,
                snapshot_rejected: false,
                history_floor: this.#mirror.history_floor,
            },
        };
        return done;
    }
    /** Real work: parse NDJSON, apply every storage diff, count every event. */
    #fold(payload, into) {
        const text = dec.decode(payload);
        const notes = into === 'base' ? this.#mirror.notes : this.#mirror.tailNotes;
        const nullifiers = into === 'base' ? this.#mirror.nullifiers : this.#mirror.tailNullifiers;
        let hdr = null;
        let start = 0;
        while (start < text.length) {
            let end = text.indexOf('\n', start);
            if (end < 0)
                end = text.length;
            const line = text.slice(start, end);
            start = end + 1;
            if (line.length === 0)
                continue;
            let parsed;
            try {
                parsed = JSON.parse(line);
            }
            catch {
                throw err('FEED_MALFORMED', 'feed line is not JSON');
            }
            if (parsed.t === 'hdr') {
                hdr = parsed;
                continue;
            }
            const blk = parsed;
            this.#mirror.blocks += 1;
            for (const [k, v] of blk.d ?? []) {
                this.#mirror.slots[k] = v;
                this.#mirror.slotWrites += 1;
            }
            this.#mirror.events += (blk.e ?? []).length;
            for (const n of blk.n ?? []) {
                this.#mirror.mockNotesSeen = true;
                notes.push({ ...n, b: blk.b, t: blk.ts });
            }
            // live-run §7: a spent note's storage slot is not cleared, so spentness
            // lives only in the nullifier slot.
            for (const nf of blk.x ?? [])
                nullifiers.push(nf);
        }
        return hdr;
    }
}
function makeSeal(key, nonce, plain) {
    const pt = enc.encode(JSON.stringify(plain));
    const ct = Uint8Array.from(pt);
    const ks = keyStream(key, nonce, ct.length);
    for (let i = 0; i < ct.length; i++)
        ct[i] = ct[i] ^ ks[i];
    const mac = sha256(concatBytes(enc.encode('strk20-mock-mac/'), key, nonce, ct));
    return concatBytes(enc.encode('S1'), nonce, mac, ct);
}
function openSeal(key, sealed) {
    try {
        if (sealed.length < 2 + 32 + 32)
            return null;
        if (sealed[0] !== 0x53 || sealed[1] !== 0x31)
            return null;
        const nonce = sealed.subarray(2, 34);
        const mac = sealed.subarray(34, 66);
        const ct = sealed.subarray(66);
        const want = sha256(concatBytes(enc.encode('strk20-mock-mac/'), key, nonce, ct));
        for (let i = 0; i < 32; i++)
            if (want[i] !== mac[i])
                return null;
        const ks = keyStream(key, nonce, ct.length);
        const pt = new Uint8Array(ct.length);
        for (let i = 0; i < ct.length; i++)
            pt[i] = ct[i] ^ ks[i];
        return JSON.parse(dec.decode(pt));
    }
    catch {
        return null;
    }
}
// ------------------------------------------------------------------ helpers
function u32(n) {
    const b = new Uint8Array(4);
    new DataView(b.buffer).setUint32(0, n, false);
    return b;
}
export function epochPath(e) {
    return `/epochs/${String(e).padStart(8, '0')}.strk20e.zst`;
}
function epochIndexOf(path) {
    const m = /\/epochs\/([0-9]{8})\.strk20e\.zst$/.exec(path);
    if (!m)
        throw err('FEED_MALFORMED', 'unparseable epoch path');
    return Number(m[1]);
}
function reasonFor(artifact, path) {
    if (artifact === 'epoch')
        return `epoch ${epochIndexOf(path)} above last applied`;
    if (artifact === 'head')
        return 'tail refresh';
    if (artifact === 'genesis')
        return 'chain identity check';
    return 'manifest';
}
function need(b, step) {
    if (!b)
        throw err('SYNC_PROTOCOL', 'artifact supplied with no bytes', { artifact: step.artifact });
    return b;
}
function profileHash(p) {
    return sha256Hex(enc.encode(`${p.chainId}|${p.pool}|${p.genesisBlock}|${p.epochSize}|${p.feedFormat}`));
}
function err(code, msg, details) {
    return new Strk20Error(code, msg, details);
}
// ------------------------------------------------------------------ factory
export const mockEngineFactory = {
    kind: 'mock',
    label: 'MOCK ENGINE — TypeScript stand-in, not the wasm module',
    provenance: 'Real fetches over the real feed schema, real sha256 verification of both buffers, real epoch ' +
        'hash-chain walk, real NDJSON fold applying every storage diff. NOT the wasm module: it cannot ' +
        'decrypt real notes (discovery scans a mock-only note encoding), there is no MPT or snapshot ' +
        'lane, and `verified` never rises above `replayed`. Timings are comparable to this engine only ' +
        '— never to the native 5.97 s figure.',
    async create(profileJson) {
        return new MockEngine(JSON.parse(profileJson));
    },
    async load(profileJson, frames) {
        return MockEngine.loadFrames(JSON.parse(profileJson), frames);
    },
};
//# sourceMappingURL=engine-mock.js.map