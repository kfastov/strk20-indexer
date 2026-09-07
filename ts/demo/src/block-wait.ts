import { subscribeRpc } from "./rpc-subscription.ts";

/** Check on new heads, coalescing arrivals during a slow check. No block polling. */
export function waitForBlock<T>(
  nodeUrl: string,
  getBlockNumber: () => Promise<number>,
  check: (block: number) => Promise<T | undefined>,
): Promise<T> {
  return new Promise((resolve, reject) => {
    let stop: (() => void) | undefined;
    let finished = false;
    let checking = false;
    let next: number | undefined;
    const deadline = setTimeout(() => finish(new Error(
      "Notes have not become spendable yet. Retry after more blocks.",
    )), 300_000);
    function finish(error?: unknown, result?: T) {
      if (finished) return;
      finished = true;
      clearTimeout(deadline);
      stop?.();
      if (error) reject(error);
      else resolve(result!);
    }
    async function drain() {
      if (checking || finished) return;
      checking = true;
      try {
        while (next !== undefined && !finished) {
          const block = next;
          next = undefined;
          const result = await check(block);
          if (result !== undefined) finish(undefined, result);
        }
      } catch (error) { finish(error); }
      finally { checking = false; }
    }
    function head(block: number) {
      if (!Number.isSafeInteger(block) || block < 0) {
        finish(new Error("Invalid block header from subscription."));
        return;
      }
      next = block;
      void drain();
    }
    let receivedHead = false;
    let headGeneration = 0;
    stop = subscribeRpc({
      url: nodeUrl, method: "starknet_subscribeNewHeads", params: {}, idleMs: 30_000,
      onEvent: (event) => {
        if (event.method === "starknet_subscriptionNewHeads") {
          receivedHead = true;
          headGeneration++;
          head(event.params.result.block_number);
        } else if (event.method === "starknet_subscriptionReorg") {
          finish(new Error("Chain reorganized while waiting for notes. Retry discovery."));
        }
      },
      onReady: (reconnected, current) => {
        const before = headGeneration;
        if (!receivedHead || reconnected) void getBlockNumber().then((block) => {
          if (current() && !finished && headGeneration === before) head(block);
        }, (error) => { if (current() && !finished && headGeneration === before) finish(error); });
        receivedHead = false;
      },
      onError: finish,
    });
  });
}
