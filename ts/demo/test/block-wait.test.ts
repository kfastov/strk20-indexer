import { test, type TestContext } from "node:test";
import assert from "node:assert/strict";
import { once } from "node:events";
import { WebSocketServer, type WebSocket } from "ws";
import { waitForBlock } from "../src/block-wait.ts";

const head = (socket: WebSocket, block: number) => socket.send(JSON.stringify({
  jsonrpc: "2.0", method: "starknet_subscriptionNewHeads",
  params: { subscription_id: "sub", result: { block_number: block } },
}));
async function server(t: TestContext, subscribe: (socket: WebSocket) => void) {
  const wss = new WebSocketServer({ port: 0, host: "127.0.0.1" });
  wss.on("connection", (socket) => socket.on("message", (bytes) => {
    const request = JSON.parse(String(bytes));
    assert.equal(request.method, "starknet_subscribeNewHeads");
    subscribe(socket);
  }));
  await once(wss, "listening");
  t.after(async () => {
    for (const socket of wss.clients) socket.terminate();
    await new Promise<void>((resolve) => wss.close(() => resolve()));
  });
  const address = wss.address();
  assert(address && typeof address !== "string");
  return `ws://127.0.0.1:${address.port}`;
}
const ack = (socket: WebSocket) => socket.send(JSON.stringify({ jsonrpc: "2.0", id: 1, result: "sub" }));

test("heads carry block numbers without polling, including events before ack", async (t) => {
  let reads = 0;
  const url = await server(t, (socket) => { head(socket, 42); ack(socket); });
  assert.equal(await waitForBlock(url, async () => { reads++; return 41; }, async (block) => block), 42);
  assert.equal(reads, 0);
});

test("slow discovery coalesces heads and a late catch-up cannot replace the live head", async (t) => {
  let socket!: WebSocket;
  let finishRead!: (block: number) => void;
  let release!: () => void;
  let started!: () => void;
  const checking = new Promise<void>((resolve) => { started = resolve; });
  const held = new Promise<void>((resolve) => { release = resolve; });
  const blocks: number[] = [];
  const url = await server(t, (ws) => { socket = ws; ack(ws); });
  const result = waitForBlock(url, () => new Promise<number>((resolve) => {
    finishRead = resolve;
    head(socket, 42);
  }), async (block) => {
    blocks.push(block);
    if (block === 42) { started(); await held; return undefined; }
    return block;
  });
  await checking;
  head(socket, 43);
  head(socket, 44);
  // The ordered marker arrives after both head notifications have been dispatched.
  await new Promise<void>((resolve) => {
    socket.ping();
    socket.once("pong", () => { finishRead(41); release(); resolve(); });
  });
  assert.equal(await result, 44);
  assert.deepEqual(blocks, [42, 44]);
});

test("reconnect catches up a block missed offline", async (t) => {
  let reads = 0;
  let socketLost = false;
  let latest!: WebSocket;
  const reconnectUrl = await server(t, (socket) => { latest = socket; ack(socket); });
  assert.equal(await waitForBlock(reconnectUrl, async () => ++reads === 1 ? 41 : 42,
    async (block) => {
      if (!socketLost) { socketLost = true; latest.terminate(); return undefined; }
      return block;
    }), 42);
  assert.equal(reads, 2);
});

test("silent subscription times out without periodic reads or discovery", async (t) => {
  let checked!: () => void;
  const first = new Promise<void>((resolve) => { checked = resolve; });
  let reads = 0, checks = 0;
  const url = await server(t, ack);
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const result = assert.rejects(waitForBlock(url, async () => { reads++; return 41; }, async () => {
    checks++; checked(); return undefined;
  }), /not become spendable/);
  await first;
  t.mock.timers.tick(300_000);
  await result;
  t.mock.timers.reset();
  assert.equal(reads, 1);
  assert.equal(checks, 1);
});

test("subscription rejection fails promptly", async (t) => {
  const url = await server(t, (socket) => socket.send(JSON.stringify({
    jsonrpc: "2.0", id: 1, error: { code: -32601, message: "unsupported" },
  })));
  await assert.rejects(waitForBlock(url, async () => 42, async (block) => block), /unsupported/);
});
