import type { RpcProvider } from "starknet";

type ReceiptResponse = Awaited<ReturnType<RpcProvider["getTransactionReceipt"]>>;
type AcceptedReceipt = Extract<ReceiptResponse, { transaction_hash: string }>;
const accepted = (status: string) => ["ACCEPTED_ON_L2", "ACCEPTED_ON_L1"].includes(status);

/** Catch-up on connect and terminal WS failure; otherwise status-driven reads. */
export function waitForReceipt(
  hash: string, nodeUrl: string, getReceipt: () => Promise<ReceiptResponse>,
): Promise<AcceptedReceipt> {
  return new Promise((resolve, reject) => {
    let socket: WebSocket;
    let finished = false;
    let reading: Promise<void> | undefined;
    let checkingFailure = false;
    let acceptedWhileReading = false;
    let retries = 0;
    let reconnect: ReturnType<typeof setTimeout> | undefined;
    let catchupDeadline: ReturnType<typeof setTimeout> | undefined;
    const deadline = setTimeout(() => void failAfterCatchup(new Error(
      "Confirmation timed out. Resume the saved transaction; do not send again.",
    )), 120_000);
    function finish(error?: unknown, receipt?: AcceptedReceipt) {
      if (finished) return;
      finished = true;
      clearTimeout(deadline);
      clearTimeout(reconnect);
      clearTimeout(catchupDeadline);
      socket?.close();
      if (error) reject(error);
      else resolve(receipt!);
    }
    async function failAfterCatchup(error: unknown) {
      if (finished || checkingFailure) return;
      checkingFailure = true;
      clearTimeout(deadline);
      clearTimeout(reconnect);
      catchupDeadline = setTimeout(() => finish(error), 15_000);
      socket?.close();
      // A subscription can time out after the transaction was accepted. Check
      // its saved hash once before reporting a transport failure, never resend.
      await reading;
      if (finished) return;
      await readReceipt();
      if (!finished) finish(error);
    }
    async function readReceipt(expected = false) {
      if (finished) return;
      if (reading) { acceptedWhileReading ||= expected; return reading; }
      reading = (async () => {
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
        }
      })();
      try { await reading; }
      finally {
        reading = undefined;
        if (acceptedWhileReading && !checkingFailure) {
          acceptedWhileReading = false;
          void readReceipt(true);
        }
      }
    }
    function connect() {
      if (finished || checkingFailure) return;
      try { socket = new WebSocket(nodeUrl); }
      catch (error) { void failAfterCatchup(error); return; }
      const connection = socket;
      let lost = false;
      socket.onopen = () => {
        if (finished || lost || checkingFailure) { connection.close(); return; }
        connection.send(JSON.stringify({ jsonrpc: "2.0", id: 1,
          method: "starknet_subscribeTransactionStatus", params: { transaction_hash: hash } }));
        // Catch up confirmations missed during a disconnected interval.
        if (retries > 0) void readReceipt();
      };
      // Installed before open: the initial status can precede the subscription ack.
      socket.onmessage = (message) => {
        if (finished || checkingFailure) return;
        try {
          const notification = JSON.parse(String(message.data));
          if (notification.error) {
            void failAfterCatchup(new Error(`Status subscription failed: ${notification.error.message}. Resume the saved transaction.`));
            return;
          }
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
        if (finished || lost || checkingFailure) return;
        lost = true;
        connection.close();
        if (retries === 3) void failAfterCatchup(new Error("Confirmation connection lost. Resume the saved transaction."));
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
