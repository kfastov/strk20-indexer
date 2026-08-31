/**
 * `LocalDiscoveryProvider` — the first identifier in the README and the one our
 * actual customer types.
 *
 * §4.1's positioning fact, verified from the official Wallet API docs: *"No
 * viewing keys in your app. The wallet holds the user's viewing key"* and *"The
 * wallet discovers notes, builds the proof"*. A dapp on the Wallet API never
 * receives a viewing key and therefore can neither use nor need this package.
 * Our customer is a WALLET or an app with its own keystore, and its integration
 * is one field:
 *
 *   createPrivateTransfers({ discoveryProvider: new LocalDiscoveryProvider({...}) })
 *
 * Cursor-conversion semantics carry over verbatim from base §12.1, so
 * `NotesCursor` / `ChannelCursor` round-trip identically to
 * `IndexerDiscoveryProvider`.
 */
import { KeylessClient, type KeylessClientOptions } from './client.ts';
import type { Account, DiscoveryProvider, HistoryTx, Note, NotesResult } from './types.ts';
export interface LocalDiscoveryProviderOptions extends KeylessClientOptions {
    account: Account;
}
export declare class LocalDiscoveryProvider implements DiscoveryProvider {
    #private;
    readonly client: KeylessClient;
    constructor(opts: LocalDiscoveryProviderOptions);
    notes(): Promise<NotesResult>;
    getIncomingNotes(): Promise<{
        notes: Note[];
        cursor: string | null;
        complete: boolean;
    }>;
    getOutgoingNotes(): Promise<{
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
    close(): Promise<void>;
}
//# sourceMappingURL=provider.d.ts.map