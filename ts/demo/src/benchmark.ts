import {
  IndexerDiscoveryProvider,
  type DiscoveryProviderInterface,
} from "@starkware-libs/starknet-privacy-sdk";
import type { Wallet } from "./wallet.ts";
import { NETWORKS } from "./network.ts";

type Notes = Awaited<ReturnType<DiscoveryProviderInterface["discoverNotes"]>>;
export interface Observation {
  source: "Local" | "Official";
  block: number;
  ms: number;
  attempts: number;
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
  local: DiscoveryProviderInterface,
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
    provider: DiscoveryProviderInterface,
  ): Promise<Notes> => {
    const started = performance.now();
    let attempts = 0;
    try {
      for (;;) {
        attempts++;
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
            attempts,
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
          await new Promise((resolve) => setTimeout(resolve, 1000));
        }
      }
    } catch (error) {
      result.rows.push({
        source,
        block,
        ms: performance.now() - started,
        attempts,
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
          `${token}:${note.id}:${note.amount}:${note.witness.channelKey}:${note.witness.nonce}:${note.witness.r}`,
      ),
    )
    .sort()
    .join("\n");
}
