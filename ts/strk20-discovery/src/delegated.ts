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

import { Strk20Error } from './errors.ts';
import { delegatedPost, healthGet, resolveFetch, type FetchLike } from './net.ts';
import { resolveProfile } from './profiles.ts';
import type {
  Account,
  ChainProfile,
  ClientStatus,
  DiscoveryClient,
  DiscoveryEvent,
  DiscoveryProvider,
  FeedState,
  HistoryTx,
  NetworkSummary,
  NotesResult,
  Progress,
  RequestRecord,
  Subscription,
} from './types.ts';

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

export function assertSecureServerUrl(serverUrl: string, allowInsecure: boolean): URL {
  let u: URL;
  try {
    u = new URL(serverUrl);
  } catch {
    throw new Strk20Error('CONFIG_INVALID', 'serverUrl is not a URL', { option: 'serverUrl' });
  }
  const loopback = u.hostname === 'localhost' || u.hostname === '127.0.0.1' || u.hostname === '[::1]' || u.hostname === '::1';
  if (u.protocol !== 'https:' && !loopback && !allowInsecure) {
    throw new Strk20Error('CONFIG_INVALID', 'refusing to send a viewing key over plaintext to a non-loopback host', {
      option: 'serverUrl',
      reason: 'plaintext non-loopback',
    });
  }
  return u;
}

export class DelegatedClient implements DiscoveryClient {
  readonly #opts: DelegatedClientOptions;
  readonly #profile: ChainProfile;
  #records: RequestRecord[] = [];
  #checked = false;

  constructor(opts: DelegatedClientOptions) {
    assertSecureServerUrl(opts.serverUrl, opts.allowInsecureServer ?? false);
    this.#profile = resolveProfile(opts.network);
    this.#opts = opts;
  }

  /**
   * Reads `/health` and verifies chain identity BEFORE any key is sent. Absent
   * `chain_id` / `pool` fields are a refusal unless `assertUncheckedNetwork`.
   */
  async verifyChainIdentity(): Promise<void> {
    const body = await healthGet(this.#ctx(), this.#opts.serverUrl);
    if (body.chain_id == null || body.pool == null) {
      if (!this.#opts.assertUncheckedNetwork) {
        throw new Strk20Error('CHAIN_MISMATCH', '/health declares no chain identity', {
          field: 'chain_id',
          expected: this.#profile.chainId,
          got: null,
        });
      }
    } else if (body.chain_id !== this.#profile.chainId || body.pool !== this.#profile.pool) {
      throw new Strk20Error('CHAIN_MISMATCH', 'server serves a different chain or pool', {
        expected: this.#profile.chainId,
        got: body.chain_id,
      });
    }
    this.#checked = true;
  }

  async sync(_opts?: { signal?: AbortSignal; onProgress?: (p: Progress) => void }): Promise<FeedState> {
    throw this.#notBuilt();
  }

  async getNotes(account: Account): Promise<NotesResult> {
    if (!this.#checked) await this.verifyChainIdentity();
    // Deliberately unreachable in this tree, and deliberately NOT stubbed with
    // fake notes: a delegated client that returns plausible data without a
    // server is the exact shape of a demo that lies.
    void account;
    throw this.#notBuilt();
  }

  watch(_a: Account, cb: (ev: DiscoveryEvent) => void): Subscription {
    queueMicrotask(() => cb({ type: 'error', error: this.#notBuilt(), recovering: false }));
    return { close(): void {}, closed: true };
  }

  async history(): Promise<{
    transactions: HistoryTx[];
    complete: boolean;
    completeFrom: number;
    registrationAvailable: boolean;
  }> {
    throw this.#notBuilt();
  }

  provider(_a: Account): DiscoveryProvider {
    const notBuilt = this.#notBuilt.bind(this);
    return {
      async getIncomingNotes() {
        throw notBuilt();
      },
      async getOutgoingNotes() {
        throw notBuilt();
      },
      async getTransactionHistory() {
        throw notBuilt();
      },
    };
  }

  status(): ClientStatus {
    return {
      mode: 'delegated',
      transport: 'polling',
      persistence: 'memory',
      persisted: false,
      persistMode: 'raw',
      blocking: false,
      leader: true,
      engineBytes: 0,
      head: 0,
      l1Accepted: 0,
      lastEpoch: -1,
      historyFrom: 0,
      verified: 'server-asserted',
      accounts: 0,
      network: { requests: this.#records.length, bytes: 0 },
      engine: {
        kind: 'mock',
        label: 'DELEGATED — the server computes',
        provenance: 'The viewing key travels to a server you run. Nothing is computed in this browser.',
      },
    };
  }

  network(): { records: readonly RequestRecord[]; summary: NetworkSummary } {
    return {
      records: this.#records,
      summary: { requests: this.#records.length, bytes: 0, byArtifact: {}, requestLogSha256: '' },
    };
  }

  async resetCache(): Promise<void> {}
  async close(): Promise<void> {}

  /** Exposed so the gate is testable without a server. */
  async post(path: string, body: unknown): Promise<{ status: number; text: string }> {
    const r = await delegatedPost(this.#ctx(), {
      serverUrl: this.#opts.serverUrl,
      path,
      body: JSON.stringify(body),
      authToken: this.#opts.authToken,
    });
    return { status: r.status, text: r.text };
  }

  #ctx() {
    return {
      fetchImpl: resolveFetch(this.#opts.fetch),
      now: () => Date.now(),
      onRecord: (rec: RequestRecord) => {
        this.#records.push(rec);
        this.#opts.onRequest?.(rec);
      },
    };
  }

  #notBuilt(): Strk20Error {
    return new Strk20Error('TRANSPORT', '`strk20-sync serve` (§A5, roadmap item 5) does not exist yet', {
      endpoint: this.#opts.serverUrl,
    });
  }
}
