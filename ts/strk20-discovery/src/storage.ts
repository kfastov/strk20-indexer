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

import { Strk20Error } from './errors.ts';

export interface StateMeta {
  frames: number;
  len: number;
  sha256: string;
  stamp: string;
  engine_version: string;
  profile_hash: string;
  written_at: number;
  source_manifest_hash: string;
}

export interface StorageAdapter {
  readonly kind: 'indexeddb' | 'memory';
  /** Why we are on this adapter, when it is not the one that was asked for. */
  readonly reason: string | null;
  open(): Promise<void>;
  metaGet<T>(key: string): Promise<T | null>;
  metaSet(key: string, value: unknown): Promise<void>;
  artifactGet(key: string): Promise<{ hash: string; zbytes: Uint8Array } | null>;
  artifactPut(key: string, value: { hash: string; zbytes: Uint8Array }): Promise<void>;
  artifactClear(): Promise<void>;
  stateGet(): Promise<{ meta: StateMeta; frames: Uint8Array[] } | null>;
  statePut(meta: StateMeta, frames: Uint8Array[]): Promise<void>;
  stateClear(): Promise<void>;
  cursorGet(keyId: string): Promise<Uint8Array | null>;
  cursorPut(keyId: string, sealed: Uint8Array): Promise<void>;
  cursorClear(): Promise<void>;
  close(): void;
}

export function dbName(chainId: string, pool: string): string {
  return `strk20-discovery:${chainId}:${pool}`;
}

// ------------------------------------------------------------------- memory

export class MemoryStorage implements StorageAdapter {
  readonly kind = 'memory' as const;
  readonly reason: string | null;
  #meta = new Map<string, unknown>();
  #artifacts = new Map<string, { hash: string; zbytes: Uint8Array }>();
  #state: { meta: StateMeta; frames: Uint8Array[] } | null = null;
  #cursors = new Map<string, Uint8Array>();

  constructor(reason: string | null = null) {
    this.reason = reason;
  }

  async open(): Promise<void> {}
  async metaGet<T>(key: string): Promise<T | null> {
    return (this.#meta.get(key) as T | undefined) ?? null;
  }
  async metaSet(key: string, value: unknown): Promise<void> {
    this.#meta.set(key, value);
  }
  async artifactGet(key: string) {
    return this.#artifacts.get(key) ?? null;
  }
  async artifactPut(key: string, value: { hash: string; zbytes: Uint8Array }): Promise<void> {
    this.#artifacts.set(key, value);
  }
  async artifactClear(): Promise<void> {
    this.#artifacts.clear();
  }
  async stateGet() {
    return this.#state;
  }
  async statePut(meta: StateMeta, frames: Uint8Array[]): Promise<void> {
    this.#state = { meta, frames };
  }
  async stateClear(): Promise<void> {
    this.#state = null;
  }
  async cursorGet(keyId: string) {
    return this.#cursors.get(keyId) ?? null;
  }
  async cursorPut(keyId: string, sealed: Uint8Array): Promise<void> {
    this.#cursors.set(keyId, sealed);
  }
  async cursorClear(): Promise<void> {
    this.#cursors.clear();
  }
  close(): void {}
}

// --------------------------------------------------------------- indexeddb

const STORES = ['meta', 'artifacts', 'state', 'cursors'] as const;

function req<T>(r: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    r.onsuccess = () => resolve(r.result);
    r.onerror = () => reject(r.error ?? new Error('idb request failed'));
  });
}

export class IdbStorage implements StorageAdapter {
  readonly kind = 'indexeddb' as const;
  readonly reason = null;
  readonly name: string;
  #db: IDBDatabase | null = null;

  constructor(name: string) {
    this.name = name;
  }

  async open(): Promise<void> {
    const idb = (globalThis as { indexedDB?: IDBFactory }).indexedDB;
    if (!idb) throw new Strk20Error('INTERNAL', 'indexedDB is unavailable');
    this.#db = await new Promise<IDBDatabase>((resolve, reject) => {
      // `open` can throw synchronously — the caller catches and falls back.
      const r = idb.open(this.name, 1);
      r.onupgradeneeded = () => {
        for (const s of STORES) if (!r.result.objectStoreNames.contains(s)) r.result.createObjectStore(s);
      };
      r.onsuccess = () => resolve(r.result);
      r.onerror = () => reject(r.error ?? new Error('idb open failed'));
      // Quirk 9: another tab holds an older version. We do NOT force-close it.
      r.onblocked = () => reject(new Error('idb open blocked by another tab'));
    });
  }

  #tx(store: (typeof STORES)[number], mode: IDBTransactionMode): IDBObjectStore {
    if (!this.#db) throw new Strk20Error('INTERNAL', 'storage is not open');
    return this.#db.transaction(store, mode).objectStore(store);
  }

  async metaGet<T>(key: string): Promise<T | null> {
    return ((await req(this.#tx('meta', 'readonly').get(key))) as T | undefined) ?? null;
  }
  async metaSet(key: string, value: unknown): Promise<void> {
    await req(this.#tx('meta', 'readwrite').put(value as never, key));
  }
  async artifactGet(key: string) {
    const v = (await req(this.#tx('artifacts', 'readonly').get(key))) as
      | { hash: string; zbytes: ArrayBuffer }
      | undefined;
    return v ? { hash: v.hash, zbytes: new Uint8Array(v.zbytes) } : null;
  }
  async artifactPut(key: string, value: { hash: string; zbytes: Uint8Array }): Promise<void> {
    await req(
      this.#tx('artifacts', 'readwrite').put(
        { hash: value.hash, zbytes: toArrayBuffer(value.zbytes) } as never,
        key,
      ),
    );
  }
  async artifactClear(): Promise<void> {
    await req(this.#tx('artifacts', 'readwrite').clear());
  }

  async stateGet(): Promise<{ meta: StateMeta; frames: Uint8Array[] } | null> {
    const meta = (await req(this.#tx('state', 'readonly').get('folded/meta'))) as StateMeta | undefined;
    if (!meta) return null;
    const frames: Uint8Array[] = [];
    for (let i = 0; i < meta.frames; i++) {
      const f = (await req(this.#tx('state', 'readonly').get(`folded/${i}`))) as ArrayBuffer | undefined;
      // Quirk 8: a partial write is detectable as a frame-count mismatch and is
      // treated as a cache miss, never a corruption.
      if (!f) return null;
      frames.push(new Uint8Array(f));
    }
    return { meta, frames };
  }

  async statePut(meta: StateMeta, frames: Uint8Array[]): Promise<void> {
    if (!this.#db) throw new Strk20Error('INTERNAL', 'storage is not open');
    // Quirk 1: never await a fetch inside a transaction. Bytes are already
    // staged; this is one transaction, start to finish.
    const tx = this.#db.transaction('state', 'readwrite');
    const store = tx.objectStore('state');
    store.clear();
    frames.forEach((f, i) => store.put(toArrayBuffer(f) as never, `folded/${i}`));
    store.put(meta as never, 'folded/meta');
    await new Promise<void>((resolve, reject) => {
      tx.oncomplete = () => resolve();
      tx.onerror = () => reject(tx.error ?? new Error('idb state write failed'));
      tx.onabort = () => reject(tx.error ?? new Error('idb state write aborted'));
    });
  }

  async stateClear(): Promise<void> {
    await req(this.#tx('state', 'readwrite').clear());
  }
  async cursorGet(keyId: string): Promise<Uint8Array | null> {
    const v = (await req(this.#tx('cursors', 'readonly').get(keyId))) as
      | { sealed: ArrayBuffer }
      | undefined;
    return v ? new Uint8Array(v.sealed) : null;
  }
  async cursorPut(keyId: string, sealed: Uint8Array): Promise<void> {
    await req(
      this.#tx('cursors', 'readwrite').put(
        { sealed: toArrayBuffer(sealed), updatedAt: Date.now() } as never,
        keyId,
      ),
    );
  }
  async cursorClear(): Promise<void> {
    await req(this.#tx('cursors', 'readwrite').clear());
  }
  close(): void {
    this.#db?.close();
    this.#db = null;
  }
}

function toArrayBuffer(u: Uint8Array): ArrayBuffer {
  const out = new ArrayBuffer(u.length);
  new Uint8Array(out).set(u);
  return out;
}

/**
 * Open IndexedDB, or fall back to memory and SAY SO. Quirk 2: every failure
 * path falls back and reports through `status()`, because a wallet that
 * silently lost its cache is a wallet with a mystery cold start.
 */
export async function openStorage(
  want: 'indexeddb' | 'memory' | StorageAdapter,
  name: string,
): Promise<StorageAdapter> {
  if (typeof want === 'object') {
    await want.open();
    return want;
  }
  if (want === 'memory') return new MemoryStorage('requested');
  try {
    const s = new IdbStorage(name);
    await s.open();
    return s;
  } catch (e) {
    return new MemoryStorage(e instanceof Error ? e.message : 'indexeddb unavailable');
  }
}

export async function deleteDatabase(name: string): Promise<'deleted' | 'blocked' | 'unavailable'> {
  const idb = (globalThis as { indexedDB?: IDBFactory }).indexedDB;
  if (!idb) return 'unavailable';
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
