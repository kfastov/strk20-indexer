import { test, type TestContext } from "node:test";
import assert from "node:assert/strict";
import { once } from "node:events";
import { WebSocketServer, type WebSocket } from "ws";
import { type RpcProvider } from "starknet";
import { waitForReceipt } from "../src/confirmation.ts";

type Receipt = Awaited<ReturnType<RpcProvider["getTransactionReceipt"]>>;
const hash = "0x123";
const receipt = {
  transaction_hash: hash, block_number: 42,
  execution_status: "SUCCEEDED", finality_status: "ACCEPTED_ON_L2",
};
const status = (socket: WebSocket, finality = "ACCEPTED_ON_L2") => socket.send(JSON.stringify({
  jsonrpc: "2.0", method: "starknet_subscriptionTransactionStatus",
  params: { subscription_id: "sub", result: {
    transaction_hash: hash, status: { finality_status: finality, execution_status: "SUCCEEDED" },
  } },
}));
async function server(t: TestContext, connect: (socket: WebSocket) => void) {
  const wss = new WebSocketServer({ port: 0, host: "127.0.0.1" });
  wss.on("connection", connect);
  await once(wss, "listening");
  t.after(async () => {
    for (const socket of wss.clients) socket.terminate();
    await new Promise<void>((resolve) => wss.close(() => resolve()));
  });
  const address = wss.address();
  assert(address && typeof address !== "string");
  return `ws://127.0.0.1:${address.port}`;
}

test("confirmation checks once on connect, then reads only on accepted status", async (t) => {
  let socket!: WebSocket;
  let subscribed!: () => void;
  const ready = new Promise<void>((resolve) => { subscribed = resolve; });
  const methods: string[] = [];
  const url = await server(t, (ws) => {
    socket = ws;
    ws.on("message", (bytes) => {
      const request = JSON.parse(String(bytes));
      methods.push(request.method);
      if (request.method === "starknet_subscribeTransactionStatus") {
        assert.equal(request.params.transaction_hash, hash);
        ws.send(JSON.stringify({ jsonrpc: "2.0", id: request.id, result: "sub" }));
        status(ws, "PRE_CONFIRMED");
        subscribed();
      } else assert.fail(`Unexpected WebSocket method: ${request.method}`);
    });
  });
  const result = waitForReceipt(hash, url, async () => {
    methods.push("starknet_getTransactionReceipt");
    if (methods.filter((m) => m === "starknet_getTransactionReceipt").length === 1)
      throw Object.assign(new Error("Transaction hash not found"), { code: 29 });
    return receipt as Receipt;
  });
  await ready;
  assert.equal(methods.filter((m) => m === "starknet_subscribeTransactionStatus").length, 1);
  assert.equal(methods.filter((m) => m === "starknet_getTransactionReceipt").length, 1);
  const closed = once(socket, "close");
  status(socket);
  status(socket); // A duplicate notification cannot duplicate the receipt request.
  assert.equal((await result).block_number, 42);
  await closed;
  assert.equal(methods.filter((m) => m === "starknet_getTransactionReceipt").length, 2);
});

test("a dropped socket reconnects and the restored subscription reports the current status", async (t) => {
  let connections = 0;
  let reads = 0;
  const url = await server(t, (ws) => {
    const attempt = ++connections;
    ws.on("message", (bytes) => {
      const request = JSON.parse(String(bytes));
      if (request.method === "starknet_subscribeTransactionStatus") {
        ws.send(JSON.stringify({ jsonrpc: "2.0", id: request.id, result: "sub" }));
        if (attempt === 1) setImmediate(() => ws.terminate());
        else status(ws);
      } else assert.fail(`Unexpected WebSocket method: ${request.method}`);
    });
  });
  assert.equal((await waitForReceipt(hash, url, async () => {
    reads++;
    if (connections < 2) throw Object.assign(new Error("Not found"), { code: 29 });
    return receipt as Receipt;
  })).block_number, 42);
  assert.equal(connections, 2);
  assert.equal(reads, 2);
});

for (const [label, changed, expected] of [
  ["reverted receipt", { execution_status: "REVERTED" }, "REVERTED"],
  ["wrong hash", { transaction_hash: "0x456" }, null],
  ["unaccepted receipt", { finality_status: "PRE_CONFIRMED" }, null],
  ["missing block", { block_number: undefined }, null],
] as const) {
  test(`confirmation handles ${label}`, async (t) => {
    const url = await server(t, (ws) => ws.on("message", (bytes) => {
      const request = JSON.parse(String(bytes));
      if (request.method === "starknet_subscribeTransactionStatus") {
        ws.send(JSON.stringify({ jsonrpc: "2.0", id: request.id, result: "sub" }));
        status(ws); // Also covers a transaction accepted before subscription.
      } else assert.fail(`Unexpected WebSocket method: ${request.method}`);
    }));
    let reads = 0;
    const getReceipt = async () => {
      if (++reads === 1) throw Object.assign(new Error("Not found"), { code: 29 });
      return { ...receipt, ...changed } as Receipt;
    };
    if (expected) assert.equal((await waitForReceipt(hash, url, getReceipt)).execution_status, expected);
    else await assert.rejects(waitForReceipt(hash, url, getReceipt), /Invalid .*receipt/);
  });
}

test("a stalled confirmation times out without receipt polling", async (t) => {
  let subscribed!: () => void;
  const ready = new Promise<void>((resolve) => { subscribed = resolve; });
  let requests = 0, receipts = 0;
  const url = await server(t, (ws) => ws.on("message", (bytes) => {
    requests++;
    const request = JSON.parse(String(bytes));
    ws.send(JSON.stringify({ jsonrpc: "2.0", id: request.id, result: "sub" }));
    subscribed();
  }));
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const result = assert.rejects(waitForReceipt(hash, url, async () => { receipts++; throw Object.assign(new Error("Not found"), { code: 29 }); }), /Resume the saved transaction/);
  await ready;
  t.mock.timers.tick(120_000);
  await result;
  t.mock.timers.reset();
  assert.equal(requests, 1);
  assert.equal(receipts, 1);
});

test("failed WS handshakes stop after bounded reconnects and keep the pending transaction resumable", async (t) => {
  let handshakes = 0;
  const wss = new WebSocketServer({ port: 0, host: "127.0.0.1",
    verifyClient: (_info, done) => { handshakes++; done(false, 403); } });
  await once(wss, "listening");
  t.after(() => new Promise<void>((resolve) => wss.close(() => resolve())));
  const address = wss.address();
  assert(address && typeof address !== "string");
  let reads = 0;
  await assert.rejects(waitForReceipt(hash, `ws://127.0.0.1:${address.port}`, async () => {
    reads++;
    throw Object.assign(new Error("Not found"), { code: 29 });
  }), /connection lost.*Resume/);
  assert.equal(handshakes, 4);
  assert.equal(reads, 1);
});
