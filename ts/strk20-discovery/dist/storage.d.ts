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
    artifactGet(key: string): Promise<{
        hash: string;
        zbytes: Uint8Array;
    } | null>;
    artifactPut(key: string, value: {
        hash: string;
        zbytes: Uint8Array;
    }): Promise<void>;
    artifactClear(): Promise<void>;
    stateGet(): Promise<{
        meta: StateMeta;
        frames: Uint8Array[];
    } | null>;
    statePut(meta: StateMeta, frames: Uint8Array[]): Promise<void>;
    stateClear(): Promise<void>;
    cursorGet(keyId: string): Promise<Uint8Array | null>;
    cursorPut(keyId: string, sealed: Uint8Array): Promise<void>;
    cursorClear(): Promise<void>;
    close(): void;
}
export declare function dbName(chainId: string, pool: string): string;
export declare class MemoryStorage implements StorageAdapter {
    #private;
    readonly kind: "memory";
    readonly reason: string | null;
    constructor(reason?: string | null);
    open(): Promise<void>;
    metaGet<T>(key: string): Promise<T | null>;
    metaSet(key: string, value: unknown): Promise<void>;
    artifactGet(key: string): Promise<{
        hash: string;
        zbytes: Uint8Array;
    } | null>;
    artifactPut(key: string, value: {
        hash: string;
        zbytes: Uint8Array;
    }): Promise<void>;
    artifactClear(): Promise<void>;
    stateGet(): Promise<{
        meta: StateMeta;
        frames: Uint8Array[];
    } | null>;
    statePut(meta: StateMeta, frames: Uint8Array[]): Promise<void>;
    stateClear(): Promise<void>;
    cursorGet(keyId: string): Promise<Uint8Array<ArrayBufferLike> | null>;
    cursorPut(keyId: string, sealed: Uint8Array): Promise<void>;
    cursorClear(): Promise<void>;
    close(): void;
}
export declare class IdbStorage implements StorageAdapter {
    #private;
    readonly kind: "indexeddb";
    readonly reason: null;
    readonly name: string;
    constructor(name: string);
    open(): Promise<void>;
    metaGet<T>(key: string): Promise<T | null>;
    metaSet(key: string, value: unknown): Promise<void>;
    artifactGet(key: string): Promise<{
        hash: string;
        zbytes: Uint8Array<ArrayBuffer>;
    } | null>;
    artifactPut(key: string, value: {
        hash: string;
        zbytes: Uint8Array;
    }): Promise<void>;
    artifactClear(): Promise<void>;
    stateGet(): Promise<{
        meta: StateMeta;
        frames: Uint8Array[];
    } | null>;
    statePut(meta: StateMeta, frames: Uint8Array[]): Promise<void>;
    stateClear(): Promise<void>;
    cursorGet(keyId: string): Promise<Uint8Array | null>;
    cursorPut(keyId: string, sealed: Uint8Array): Promise<void>;
    cursorClear(): Promise<void>;
    close(): void;
}
/**
 * Open IndexedDB, or fall back to memory and SAY SO. Quirk 2: every failure
 * path falls back and reports through `status()`, because a wallet that
 * silently lost its cache is a wallet with a mystery cold start.
 */
export declare function openStorage(want: 'indexeddb' | 'memory' | StorageAdapter, name: string): Promise<StorageAdapter>;
export declare function deleteDatabase(name: string): Promise<'deleted' | 'blocked' | 'unavailable'>;
//# sourceMappingURL=storage.d.ts.map