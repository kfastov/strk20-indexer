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
import { KeylessClient } from "./client.js";
export class LocalDiscoveryProvider {
    client;
    #account;
    constructor(opts) {
        const { account, ...rest } = opts;
        this.client = new KeylessClient(rest);
        this.#account = account;
    }
    async notes() {
        return this.client.getNotes(this.#account);
    }
    async getIncomingNotes() {
        const r = await this.client.getNotes(this.#account);
        return { notes: r.notes.filter((n) => !n.spent), cursor: null, complete: r.complete };
    }
    async getOutgoingNotes() {
        const r = await this.client.getNotes(this.#account);
        return { notes: r.notes.filter((n) => n.spent), cursor: null, complete: r.complete };
    }
    async getTransactionHistory(opts) {
        return this.client.history(this.#account, opts);
    }
    async close() {
        await this.client.close();
    }
}
//# sourceMappingURL=provider.js.map