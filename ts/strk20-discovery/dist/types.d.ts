/**
 * Public type surface. Transcribed from consumer-path.md §4.2; where this file
 * deviates the deviation is commented and carries a reason.
 *
 * Every union here is CLOSED on purpose (§4.2 "Must not ship"): a logger or a
 * telemetry pipe attached to our event bus cannot receive key material, because
 * no member of `DiscoveryEvent` is typed as an open `string` / `unknown` that
 * could carry one. `NotesResult.raw` is the single exception and it is the
 * module's own `SyncReport`, which the Rust scanner asserts key-clean.
 */
import type { Strk20Error } from './errors.ts';
/** §A6 chain profile. One source, consumed by Rust and TypeScript. */
export interface ChainProfile {
    readonly name: string;
    readonly chainId: string;
    readonly pool: `0x${string}`;
    readonly genesisBlock: number;
    readonly epochSize: number;
    readonly feedFormat: number;
}
/**
 * An owner the client can discover for. The client NEVER stores the key: it
 * calls `viewingKey()` at the start of every pass and zeroizes the bytes it was
 * given before the pass returns. A locked wallet rejects, and the client
 * reports `{type:'status', state:'locked'}` rather than failing the session.
 */
export interface Account {
    readonly address: `0x${string}`;
    /**
     * 32-byte big-endian viewing key. Return a FRESH array each call — the client
     * zeroizes it. Reject to decline (locked, denied, revoked).
     */
    viewingKey(): Promise<Uint8Array>;
}
export interface Note {
    token: string;
    index: number;
    noteId: string;
    nullifier: string;
    amount: bigint;
    blockNumber: number;
    blockTimestamp: number;
    sender: string;
    spent: boolean;
}
export interface HistoryTx {
    kind: 'deposit' | 'transfer' | 'withdraw' | 'registration';
    blockNumber: number;
    blockTimestamp: number;
    token: string;
    amount: bigint;
    noteId: string | null;
    nullifier: string | null;
}
export type Phase = 'idle' | 'open' | 'manifest' | 'snapshot' | 'epochs' | 'head' | 'anchor' | 'persist' | 'discover';
export interface Progress {
    phase: Phase;
    done: number;
    total: number;
    bytes: number;
    requests: number;
    elapsedMs: number;
}
export interface SyncTiming {
    totalMs: number;
    phases: {
        open: number;
        manifest: number;
        fetch: number;
        decompress: number;
        apply: number;
        load: number;
        export: number;
        anchor: number;
        discover: number;
    };
    cold: boolean;
    fromCache: 'folded' | 'raw' | 'none';
}
export type RequestArtifact = 'genesis' | 'manifest' | 'epoch' | 'epoch_anchor' | 'snapshot' | 'snapshot_anchor' | 'anchors' | 'head' | 'live' | 'rpc';
export interface RequestRecord {
    /** Absolute, exactly as issued, never truncated. */
    url: string;
    method: 'GET' | 'POST';
    purpose: 'feed' | 'live' | 'anchor-rpc';
    artifact: RequestArtifact;
    status: number;
    /** Response body bytes actually received. */
    bytes: number;
    /** PerformanceResourceTiming.transferSize, or null when unavailable. */
    transferBytes: number | null;
    /** 0 for every feed request, by construction. */
    requestBodyBytes: number;
    source: 'network' | 'etag-304' | 'idb-cache';
    ms: number;
    at: number;
}
export interface NetworkSummary {
    requests: number;
    bytes: number;
    byArtifact: Record<string, {
        requests: number;
        bytes: number;
    }>;
    /** Computed INSIDE the module (§3.3), not from this UI-side list. */
    requestLogSha256: string;
}
export interface FeedState {
    head: number;
    l1Accepted: number;
    lastEpoch: number;
    lastEpochTo: number;
    historyFrom: number;
    snapshotBasis: number | null;
    snapshotRejected: boolean;
    verified: 'anchored' | 'server-asserted' | 'replayed';
    staleness: 'ok' | 'behind' | 'diverged';
    changed: boolean;
    cold: boolean;
    timing: SyncTiming;
    network: NetworkSummary;
}
export interface NotesResult {
    notes: Note[];
    balances: Map<string, bigint>;
    added: Note[];
    spent: Note[];
    feed: FeedState;
    complete: boolean;
    historyFrom: number;
    cursorReset: boolean;
    stats: {
        slotsRead: number;
        eventsScanned: number;
        passesIn: number;
        passesOut: number;
    };
    /** Discovery only. EXCLUDES the feed pass. */
    elapsedMs: number;
    /** The untouched SyncReport (oracle equality). */
    raw: unknown;
}
export type DiscoveryEvent = {
    type: 'progress';
    progress: Progress;
} | {
    type: 'feed';
    feed: FeedState;
} | {
    type: 'notes';
    added: Note[];
    spent: Note[];
    balances: Map<string, bigint>;
    head: number;
    elapsedMs: number;
} | {
    type: 'reorg';
    rewoundTo: number;
} | {
    type: 'status';
    state: 'live' | 'polling' | 'degraded' | 'locked' | 'idle';
} | {
    type: 'request';
    record: RequestRecord;
} | {
    type: 'error';
    error: Strk20Error;
    recovering: boolean;
};
export interface Subscription {
    close(): void;
    readonly closed: boolean;
}
export interface ClientStatus {
    mode: 'keyless' | 'delegated';
    transport: 'sse' | 'polling';
    persistence: 'indexeddb' | 'memory';
    /** navigator.storage.persisted() */
    persisted: boolean;
    persistMode: 'raw' | 'folded' | 'both';
    /** true when worker:false — work runs on the caller's thread */
    blocking: boolean;
    /** this tab owns the SSE connection (§4.11) */
    leader: boolean;
    /** wasm linear memory currently held, or 0 when the engine cannot report it */
    engineBytes: number;
    head: number;
    l1Accepted: number;
    lastEpoch: number;
    historyFrom: number;
    verified: 'anchored' | 'server-asserted' | 'replayed';
    accounts: number;
    network: {
        requests: number;
        bytes: number;
    };
    /**
     * NOT in §4.2. Added because this build ships two engine adapters and a
     * screenshot must never be able to misrepresent which one produced a number.
     * `kind: 'mock'` means the wasm module was not the computer.
     */
    engine: {
        kind: 'wasm' | 'mock';
        label: string;
        provenance: string;
    };
}
/** The upstream SDK socket (`createPrivateTransfers({ discoveryProvider })`). */
export interface DiscoveryProvider {
    getIncomingNotes(cursor?: string | null): Promise<{
        notes: Note[];
        cursor: string | null;
        complete: boolean;
    }>;
    getOutgoingNotes(cursor?: string | null): Promise<{
        notes: Note[];
        cursor: string | null;
        complete: boolean;
    }>;
    getTransactionHistory(opts?: {
        fromBlock?: number;
        limit?: number;
    }): Promise<{
        transactions: HistoryTx[];
        complete: boolean;
        completeFrom: number;
        registrationAvailable: boolean;
    }>;
}
export interface DiscoveryClient {
    /**
     * Bring the local mirror to the feed's head. Takes NO key and emits no
     * key-derived value.
     */
    sync(opts?: {
        signal?: AbortSignal;
        onProgress?: (p: Progress) => void;
    }): Promise<FeedState>;
    getNotes(a: Account, opts?: {
        signal?: AbortSignal;
        onProgress?: (p: Progress) => void;
        /** 'none' = discover over the mirror as it is. */
        refresh?: 'auto' | 'force' | 'none';
    }): Promise<NotesResult>;
    watch(a: Account, cb: (ev: DiscoveryEvent) => void): Subscription;
    history(a: Account, opts?: {
        fromBlock?: number;
        limit?: number;
        signal?: AbortSignal;
    }): Promise<{
        transactions: HistoryTx[];
        complete: boolean;
        completeFrom: number;
        registrationAvailable: boolean;
    }>;
    provider(a: Account): DiscoveryProvider;
    status(): ClientStatus;
    network(): {
        records: readonly RequestRecord[];
        summary: NetworkSummary;
    };
    resetCache(opts?: {
        identities?: boolean;
    }): Promise<void>;
    close(): Promise<void>;
}
//# sourceMappingURL=types.d.ts.map