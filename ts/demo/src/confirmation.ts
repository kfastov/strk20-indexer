import type { RpcProvider } from "starknet";

type ReceiptResponse = Awaited<ReturnType<RpcProvider["getTransactionReceipt"]>>;
type AcceptedReceipt = Extract<ReceiptResponse, { transaction_hash: string }>;
const accepted = (status: string) => ["ACCEPTED_ON_L2", "ACCEPTED_ON_L1"].includes(status);

/** One catch-up read on connect, then status-driven receipt reads. No polling. */
export function waitForReceipt(
  hash: string, nodeUrl: string, getReceipt: () => Promise<ReceiptResponse>,
): Promise<AcceptedReceipt> {
  return new Promise((resolve, reject) => {
    let socket: WebSocket;
    let finished = false;
    let reading = false;
    let acceptedWhileReading = false;
    let retries = 0;
    let reconnect: ReturnType<typeof setTimeout> | undefined;
    const deadline = setTimeout(() => finish(new Error(
      "Confirmation timed out. Resume the saved transaction; do not send again.",
    )), 120_000);
    function finish(error?: unknown, receipt?: AcceptedReceipt) {
      if (finished) return;
      finished = true;
      clearTimeout(deadline);
      clearTimeout(reconnect);
      socket?.close();
      if (error) reject(error);
      else resolve(receipt!);
    }
    async function readReceipt(expected = false) {
      if (finished) return;
      if (reading) { acceptedWhileReading ||= expected; return; }
      reading = true;
      try {
        const receipt = await getReceipt();
        if (!("transaction_hash" in receipt) || BigInt(receipt.transaction_hash) !== BigInt(hash)) {
          throw new Error("Invalid receipt hash. Resume the saved transaction.");
        }
        if (!expected && !accepted(receipt.finality_status)) return;
        if (!Number.isSafeInteger(receipt.block_number) || receipt.block_number < 0 ||
            !accepted(receipt.finality_status) ||
            !["SUCCEEDED", "REVERTED"].includes(receipt.execution_status)) {
          throw new Error("Invalid accepted receipt. Resume the saved transaction.");
        }
        finish(undefined, receipt);
      } catch (error) {
        // A newly sent transaction may not be known at the initial catch-up read.
        if (expected || !(error instanceof Error && "code" in error && error.code === 29)) finish(error);
      } finally {
        reading = false;
        if (acceptedWhileReading) {
          acceptedWhileReading = false;
          void readReceipt(true);
        }
      }
    }
    function connect() {
      if (finished) return;
      try { socket = new WebSocket(nodeUrl); }
      catch (error) { finish(error); return; }
      const connection = socket;
      let lost = false;
      socket.onopen = () => {
        if (finished || lost) { connection.close(); return; }
        connection.send(JSON.stringify({ jsonrpc: "2.0", id: 1,
          method: "starknet_subscribeTransactionStatus", params: { transaction_hash: hash } }));
        // Catch up confirmations missed during a disconnected interval.
        if (retries > 0) void readReceipt();
      };
      // Installed before open: the initial status can precede the subscription ack.
      socket.onmessage = (message) => {
        try {
          const notification = JSON.parse(String(message.data));
          if (notification.error) throw new Error(`Status subscription failed: ${notification.error.message}`);
          if (notification.method !== "starknet_subscriptionTransactionStatus") return;
          const event = notification.params.result;
          if (BigInt(event.transaction_hash) !== BigInt(hash)) return;
          if (accepted(event.status.finality_status)) void readReceipt(true);
          else if (event.status.finality_status === "REJECTED") {
            finish(new Error("Transaction rejected. Inspect the saved transaction before retrying."));
          }
        } catch (error) { finish(error); }
      };
      const disconnected = () => {
        if (finished || lost) return;
        lost = true;
        connection.close();
        if (retries === 3) finish(new Error("Confirmation connection lost. Resume the saved transaction."));
        else reconnect = setTimeout(connect, 250 * 2 ** retries++);
      };
      socket.onclose = disconnected;
      socket.onerror = disconnected;
    }
    connect();
    // Restoring an accepted transaction must not wait for the WS handshake.
    void readReceipt();
  });
}
