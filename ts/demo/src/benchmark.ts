import {
  IndexerDiscoveryProvider,
  type DiscoveryProviderInterface,
} from "strk20-discovery/privacy-sdk";
import type { Wallet } from "./wallet.ts";
import { NETWORKS } from "./network.ts";

type Notes = Awaited<ReturnType<DiscoveryProviderInterface["discoverNotes"]>>;
type NotesProvider = Pick<DiscoveryProviderInterface, "discoverNotes"> & {
  waitForBlock?: (block: number) => Promise<void>;
};
export interface Observation {
  source: "Local" | "Official";
  block: number;
  ms: number;
  attempts: number;
  startedAt: number;
  retries: { error: string; attemptMs: number; waitMs: number; waitFor: "feed" | "backoff" }[];
  error?: string;
}
export interface Comparison {
  block: number;
  rows: Observation[];
  cacheConditions: string;
  equal?: boolean;
}
export async function observeAt(
  wallet: Wallet,
  local: NotesProvider,
  block: number,
  compare: boolean,
  onUpdate: (result: Comparison) => void,
): Promise<Notes> {
  const result: Comparison = {
    block,
    rows: [],
    cacheConditions:
      "Local uses saved state when available; official starts without a supplied cursor. Same target block, not identical cache conditions.",
  };
  const observe = async (
    source: Observation["source"],
    provider: NotesProvider,
  ): Promise<Notes> => {
    const started = performance.now();
    const startedAt = Date.now();
    const retries: Observation["retries"] = [];
    let attempts = 0;
    try {
      for (;;) {
        attempts++;
        const attemptStarted = performance.now();
        try {
          const found = await provider.discoverNotes(
            BigInt(wallet.address),
            BigInt(wallet.viewingKey),
            { blockIdentifier: block },
          );
          if (found.timestamp !== block)
            throw new Error("Observer returned another block.");
          result.rows.push({
            source,
            block,
            ms: performance.now() - started,
            attempts, startedAt, retries,
          });
          onUpdate(result);
          return found;
        } catch (error) {
          if (
            performance.now() - started >= 120_000 ||
            !/BOUND_UNAVAILABLE|CHECKPOINT_UNAVAILABLE|RPC_UNAVAILABLE|not found|not yet|behind|HTTP 50/i.test(
              String(error),
            )
          )
            throw error;
          const attemptMs = performance.now() - attemptStarted;
          const waitStarted = performance.now();
          const waitFor = provider.waitForBlock && /BOUND_UNAVAILABLE/.test(String(error)) ? "feed" : "backoff";
          if (waitFor === "feed")
            await provider.waitForBlock!(block);
          else
            // Reference API and transient transport failures have no readiness
            // subscription. This backoff is not used for our feed availability.
            await new Promise((resolve) => setTimeout(resolve, 1000));
          retries.push({ error: String(error), attemptMs, waitMs: performance.now() - waitStarted, waitFor });
        }
      }
    } catch (error) {
      result.rows.push({
        source,
        block,
        ms: performance.now() - started,
        attempts, startedAt, retries,
        error: String(error),
      });
      onUpdate(result);
      throw error;
    }
  };
  // The second service only receives a viewing key after the explicit UI opt-in.
  const reference = compare
    ? new IndexerDiscoveryProvider(
        NETWORKS[wallet.network].reference,
        wallet.pool,
      )
    : undefined;
  const jobs = [
    observe("Local", local),
    ...(reference ? [observe("Official", reference)] : []),
  ];
  const [ours, theirs] = await Promise.allSettled(jobs);
  if (ours!.status === "rejected") throw ours!.reason;
  if (theirs?.status === "fulfilled") {
    result.equal = signature(ours!.value) === signature(theirs.value);
    onUpdate(result);
  }
  return ours!.value;
}
function signature(result: Notes): string {
  return [...result.notes.entries()]
    .flatMap(([token, notes]) =>
      notes.map(
        (note) =>
          `${token}:${BigInt(note.id)}:${note.amount}:${note.witness.channelKey}:${note.witness.nonce}:${note.witness.r}`,
      ),
    )
    .sort()
    .join("\n");
}
