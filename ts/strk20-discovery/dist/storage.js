/**
 * §4.4's IndexedDB layout, and a memory adapter with the same surface.
 *
 * Database name `strk20-discovery:<chain_id>:<pool>` — per-chain-and-pool, so
 * cross-network confusion is impossible rather than detected.
 *
 *   meta       string                          format_v, last_epoch, genesis bytes, persist mode
 *   artifacts  "snapshot" | "anchor" | epoch#   {hash, zbytes} — compressed EXACTLY as served
 *   state      "folded/meta" | "folded/<i>"     ≤4 MiB frames
 *   cursors    keyId (64 lowercase hex)         {sealed, updatedAt}
 *
 * `artifacts` values stay compressed exactly as served, because Design R's whole
 * point is that a reload re-runs the same verification ladder over the same
 * bytes the network would have delivered.
 *
 * Never stored: head.ndjson bytes, the head ETag, anything tail-derived.
 */
import { Strk20Error } from "./errors.js";
export function dbName(chainId, pool) {
    return `strk20-discovery:${chainId}:${pool}`;
}
// ------------------------------------------------------------------- memory
export class MemoryStorage {
    kind = 'memory';
    reason;
    #meta = new Map();
    #artifacts = new Map();
    #state = null;
    #cursors = new Map();
    constructor(reason = null) {
        this.reason = reason;
    }
    async open() { }
    async metaGet(key) {
        return this.#meta.get(key) ?? null;
    }
    async metaSet(key, value) {
        this.#meta.set(key, value);
    }
    async artifactGet(key) {
        return this.#artifacts.get(key) ?? null;
    }
    async artifactPut(key, value) {
        this.#artifacts.set(key, value);
    }
    async artifactClear() {
        this.#artifacts.clear();
    }
    async stateGet() {
        return this.#state;
    }
    async statePut(meta, frames) {
        this.#state = { meta, frames };
    }
    async stateClear() {
        this.#state = null;
    }
    async cursorGet(keyId) {
        return this.#cursors.get(keyId) ?? null;
    }
    async cursorPut(keyId, sealed) {
        this.#cursors.set(keyId, sealed);
    }
    async cursorClear() {
        this.#cursors.clear();
    }
    close() { }
}
// --------------------------------------------------------------- indexeddb
const STORES = ['meta', 'artifacts', 'state', 'cursors'];
function req(r) {
    return new Promise((resolve, reject) => {
        r.onsuccess = () => resolve(r.result);
        r.onerror = () => reject(r.error ?? new Error('idb request failed'));
    });
}
export class IdbStorage {
    kind = 'indexeddb';
    reason = null;
    name;
    #db = null;
    constructor(name) {
        this.name = name;
    }
    async open() {
        const idb = globalThis.indexedDB;
        if (!idb)
            throw new Strk20Error('INTERNAL', 'indexedDB is unavailable');
        this.#db = await new Promise((resolve, reject) => {
            // `open` can throw synchronously — the caller catches and falls back.
            const r = idb.open(this.name, 1);
            r.onupgradeneeded = () => {
                for (const s of STORES)
                    if (!r.result.objectStoreNames.contains(s))
                        r.result.createObjectStore(s);
            };
            r.onsuccess = () => resolve(r.result);
            r.onerror = () => reject(r.error ?? new Error('idb open failed'));
            // Quirk 9: another tab holds an older version. We do NOT force-close it.
            r.onblocked = () => reject(new Error('idb open blocked by another tab'));
        });
    }
    #tx(store, mode) {
        if (!this.#db)
            throw new Strk20Error('INTERNAL', 'storage is not open');
        return this.#db.transaction(store, mode).objectStore(store);
    }
    async metaGet(key) {
        return (await req(this.#tx('meta', 'readonly').get(key))) ?? null;
    }
    async metaSet(key, value) {
        await req(this.#tx('meta', 'readwrite').put(value, key));
    }
    async artifactGet(key) {
        const v = (await req(this.#tx('artifacts', 'readonly').get(key)));
        return v ? { hash: v.hash, zbytes: new Uint8Array(v.zbytes) } : null;
    }
    async artifactPut(key, value) {
        await req(this.#tx('artifacts', 'readwrite').put({ hash: value.hash, zbytes: toArrayBuffer(value.zbytes) }, key));
    }
    async artifactClear() {
        await req(this.#tx('artifacts', 'readwrite').clear());
    }
    async stateGet() {
        const meta = (await req(this.#tx('state', 'readonly').get('folded/meta')));
        if (!meta)
            return null;
        const frames = [];
        for (let i = 0; i < meta.frames; i++) {
            const f = (await req(this.#tx('state', 'readonly').get(`folded/${i}`)));
            // Quirk 8: a partial write is detectable as a frame-count mismatch and is
            // treated as a cache miss, never a corruption.
            if (!f)
                return null;
            frames.push(new Uint8Array(f));
        }
        return { meta, frames };
    }
    async statePut(meta, frames) {
        if (!this.#db)
            throw new Strk20Error('INTERNAL', 'storage is not open');
        // Quirk 1: never await a fetch inside a transaction. Bytes are already
        // staged; this is one transaction, start to finish.
        const tx = this.#db.transaction('state', 'readwrite');
        const store = tx.objectStore('state');
        store.clear();
        frames.forEach((f, i) => store.put(toArrayBuffer(f), `folded/${i}`));
        store.put(meta, 'folded/meta');
        await new Promise((resolve, reject) => {
            tx.oncomplete = () => resolve();
            tx.onerror = () => reject(tx.error ?? new Error('idb state write failed'));
            tx.onabort = () => reject(tx.error ?? new Error('idb state write aborted'));
        });
    }
    async stateClear() {
        await req(this.#tx('state', 'readwrite').clear());
    }
    async cursorGet(keyId) {
        const v = (await req(this.#tx('cursors', 'readonly').get(keyId)));
        return v ? new Uint8Array(v.sealed) : null;
    }
    async cursorPut(keyId, sealed) {
        await req(this.#tx('cursors', 'readwrite').put({ sealed: toArrayBuffer(sealed), updatedAt: Date.now() }, keyId));
    }
    async cursorClear() {
        await req(this.#tx('cursors', 'readwrite').clear());
    }
    close() {
        this.#db?.close();
        this.#db = null;
    }
}
function toArrayBuffer(u) {
    const out = new ArrayBuffer(u.length);
    new Uint8Array(out).set(u);
    return out;
}
/**
 * Open IndexedDB, or fall back to memory and SAY SO. Quirk 2: every failure
 * path falls back and reports through `status()`, because a wallet that
 * silently lost its cache is a wallet with a mystery cold start.
 */
export async function openStorage(want, name) {
    if (typeof want === 'object') {
        await want.open();
        return want;
    }
    if (want === 'memory')
        return new MemoryStorage('requested');
    try {
        const s = new IdbStorage(name);
        await s.open();
        return s;
    }
    catch (e) {
        return new MemoryStorage(e instanceof Error ? e.message : 'indexeddb unavailable');
    }
}
export async function deleteDatabase(name) {
    const idb = globalThis.indexedDB;
    if (!idb)
        return 'unavailable';
    return new Promise((resolve) => {
        let blocked = false;
        const r = idb.deleteDatabase(name);
        r.onsuccess = () => resolve(blocked ? 'blocked' : 'deleted');
        r.onerror = () => resolve('blocked');
        r.onblocked = () => {
            blocked = true;
        };
    });
}
//# sourceMappingURL=storage.js.map