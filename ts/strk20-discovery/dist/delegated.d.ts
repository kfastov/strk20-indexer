/**
 * `DelegatedClient` — §4.8.
 *
 * Exported from `strk20-discovery/delegated`, NOT from the package root. In
 * delegated mode the viewing key leaves the browser; that is a legitimate
 * self-host posture and a materially different trust boundary, and it should
 * not be one autocomplete away from `KeylessClient`.
 *
 * Two construction-time refusals, both of them §4.8's:
 *   - a `serverUrl` that is neither loopback nor https: is refused. A viewing
 *     key travelling in clear over a LAN is not a trade-off anyone makes
 *     deliberately.
 *   - chain identity is read from `/health` BEFORE any key is sent, and absent
 *     fields are a refusal, not a "verify if present" mode.
 *
 * STATUS IN THIS TREE: the wire calls are stubbed. `strk20-sync serve` (§A5) is
 * roadmap item 5 and does not exist yet, so `getNotes` throws TRANSPORT rather
 * than pretending. The construction-time gates below are real and tested,
 * because they are the part that protects a key.
 */
import { type FetchLike } from './net.ts';
import type { Account, ChainProfile, ClientStatus, DiscoveryClient, DiscoveryEvent, DiscoveryProvider, FeedState, HistoryTx, NetworkSummary, NotesResult, Progress, RequestRecord, Subscription } from './types.ts';
export interface DelegatedClientOptions {
    serverUrl: string;
    authToken?: string;
    network?: 'mainnet' | 'sepolia' | ChainProfile;
    assertUncheckedNetwork?: boolean;
    allowInsecureServer?: boolean;
    pollIntervalMs?: number;
    fetch?: FetchLike;
    onRequest?: (r: RequestRecord) => void;
}
export declare function assertSecureServerUrl(serverUrl: string, allowInsecure: boolean): URL;
export declare class DelegatedClient implements DiscoveryClient {
    #private;
    constructor(opts: DelegatedClientOptions);
    /**
     * Reads `/health` and verifies chain identity BEFORE any key is sent. Absent
     * `chain_id` / `pool` fields are a refusal unless `assertUncheckedNetwork`.
     */
    verifyChainIdentity(): Promise<void>;
    sync(_opts?: {
        signal?: AbortSignal;
        onProgress?: (p: Progress) => void;
    }): Promise<FeedState>;
    getNotes(account: Account): Promise<NotesResult>;
    watch(_a: Account, cb: (ev: DiscoveryEvent) => void): Subscription;
    history(): Promise<{
        transactions: HistoryTx[];
        complete: boolean;
        completeFrom: number;
        registrationAvailable: boolean;
    }>;
    provider(_a: Account): DiscoveryProvider;
    status(): ClientStatus;
    network(): {
        records: readonly RequestRecord[];
        summary: NetworkSummary;
    };
    resetCache(): Promise<void>;
    close(): Promise<void>;
    /** Exposed so the gate is testable without a server. */
    post(path: string, body: unknown): Promise<{
        status: number;
        text: string;
    }>;
}
//# sourceMappingURL=delegated.d.ts.map