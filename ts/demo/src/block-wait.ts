/** Check on new heads, coalescing arrivals during a slow check. No block polling. */
export function waitForBlock<T>(
  nodeUrl: string,
  getBlockNumber: () => Promise<number>,
  check: (block: number) => Promise<T | undefined>,
): Promise<T> {
  return new Promise((resolve, reject) => {
    let socket: WebSocket;
    let finished = false;
    let checking = false;
    let next: number | undefined;
    let retries = 0;
    let reconnect: ReturnType<typeof setTimeout> | undefined;
    const deadline = setTimeout(() => finish(new Error(
      "Notes have not become spendable yet. Retry after more blocks.",
    )), 300_000);
    function finish(error?: unknown, result?: T) {
      if (finished) return;
      finished = true;
      clearTimeout(deadline);
      clearTimeout(reconnect);
      socket?.close();
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
    function connect() {
      if (finished) return;
      try { socket = new WebSocket(nodeUrl); }
      catch (error) { finish(error); return; }
      const connection = socket;
      let lost = false;
      let receivedHead = false;
      connection.onopen = () => {
        if (finished || lost) { connection.close(); return; }
        connection.send(JSON.stringify({ jsonrpc: "2.0", id: 1,
          method: "starknet_subscribeNewHeads", params: {} }));
      };
      connection.onmessage = (message) => {
        if (finished || lost) return;
        try {
          const event = JSON.parse(String(message.data));
          if (event.error) throw new Error(`Block subscription failed: ${event.error.message}`);
          if (event.method === "starknet_subscriptionNewHeads") {
            receivedHead = true;
            head(event.params.result.block_number);
          } else if (event.id === 1) {
            // Subscribe first, then catch up once. An early/live head wins over HTTP.
            if (!receivedHead) void getBlockNumber().then((block) => {
              if (!finished && !lost && !receivedHead) head(block);
            }, (error) => { if (!finished && !lost && !receivedHead) finish(error); });
          } else if (event.method === "starknet_subscriptionReorg") {
            finish(new Error("Chain reorganized while waiting for notes. Retry discovery."));
          }
        } catch (error) { finish(error); }
      };
      const disconnected = () => {
        if (finished || lost) return;
        lost = true;
        connection.close();
        if (retries === 3) finish(new Error("Block subscription lost. Retry the action."));
        else reconnect = setTimeout(connect, 250 * 2 ** retries++);
      };
      connection.onclose = disconnected;
      connection.onerror = disconnected;
    }
    connect();
  });
}
