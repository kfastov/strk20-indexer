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

export class LocalDiscoveryProvider implements DiscoveryProvider {
  readonly client: KeylessClient;
  readonly #account: Account;

  constructor(opts: LocalDiscoveryProviderOptions) {
    const { account, ...rest } = opts;
    this.client = new KeylessClient(rest);
    this.#account = account;
  }

  async notes(): Promise<NotesResult> {
    return this.client.getNotes(this.#account);
  }

  async getIncomingNotes(): Promise<{ notes: Note[]; cursor: string | null; complete: boolean }> {
    const r = await this.client.getNotes(this.#account);
    return { notes: r.notes.filter((n) => !n.spent), cursor: null, complete: r.complete };
  }

  async getOutgoingNotes(): Promise<{ notes: Note[]; cursor: string | null; complete: boolean }> {
    const r = await this.client.getNotes(this.#account);
    return { notes: r.notes.filter((n) => n.spent), cursor: null, complete: r.complete };
  }

  async getTransactionHistory(opts?: { fromBlock?: number; limit?: number }): Promise<{
    transactions: HistoryTx[];
    complete: boolean;
    completeFrom: number;
    registrationAvailable: boolean;
  }> {
    return this.client.history(this.#account, opts);
  }

  async close(): Promise<void> {
    await this.client.close();
  }
}
