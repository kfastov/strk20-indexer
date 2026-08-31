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
import type { EngineFactory } from './engine.ts';
import { type FetchLike } from './net.ts';
import { type StorageAdapter } from './storage.ts';
import type { Account, ChainProfile, ClientStatus, DiscoveryClient, DiscoveryEvent, DiscoveryProvider, FeedState, HistoryTx, NetworkSummary, NotesResult, Progress, RequestRecord, Subscription } from './types.ts';
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
export declare class KeylessClient implements DiscoveryClient {
    #private;
    constructor(opts: KeylessClientOptions);
    get databaseName(): string;
    get profile(): ChainProfile;
    sync(opts?: {
        signal?: AbortSignal;
        onProgress?: (p: Progress) => void;
    }): Promise<FeedState>;
    getNotes(account: Account, opts?: {
        signal?: AbortSignal;
        onProgress?: (p: Progress) => void;
        refresh?: 'auto' | 'force' | 'none';
    }): Promise<NotesResult>;
    watch(account: Account, cb: (ev: DiscoveryEvent) => void): Subscription;
    history(account: Account, opts?: {
        fromBlock?: number;
        limit?: number;
        signal?: AbortSignal;
    }): Promise<{
        transactions: HistoryTx[];
        complete: boolean;
        completeFrom: number;
        registrationAvailable: boolean;
    }>;
    provider(account: Account): DiscoveryProvider;
    status(): ClientStatus;
    network(): {
        records: readonly RequestRecord[];
        summary: NetworkSummary;
    };
    /** Wasm instantiation / engine construction time, or null when not measured. */
    bootMs(): number | null;
    resetCache(opts?: {
        identities?: boolean;
    }): Promise<void>;
    close(): Promise<void>;
    /** §4 Stage 1's cold-start guard. Deleting the database is the caller's move. */
    deleteDatabase(): Promise<'deleted' | 'blocked' | 'unavailable'>;
}
//# sourceMappingURL=client.d.ts.map