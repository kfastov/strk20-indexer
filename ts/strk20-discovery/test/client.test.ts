import { test } from "node:test";
import assert from "node:assert/strict";
import { KeylessClient, type WorkerPort } from "../src/client.ts";

test("feed availability waits for a head event and close cancels the wait", async () => {
  let subscribed!: () => void;
  const subscription = new Promise<void>((resolve) => { subscribed = resolve; });
  const port: WorkerPort = {
    onmessage: null,
    onerror: null,
    postMessage(value) {
      const { id, method } = value as { id: number; method: string };
      queueMicrotask(() => {
        port.onmessage?.({ data: { id, result: {} } } as MessageEvent);
        if (method === "subscribe") subscribed();
      });
    },
    terminate() {},
  };
  const client = new KeylessClient({
    feedUrl: "https://feed.test",
    workerFactory: () => port,
  });
  let completed = false;
  const waiting = client.waitForHead(100).then(() => { completed = true; });
  await subscription;
  const head = (value: number) => port.onmessage?.({
    data: { event: "head", value },
  } as MessageEvent);
  head(99);
  await Promise.resolve();
  assert.equal(completed, false);
  head(100);
  await waiting;
  assert.equal(completed, true);
  // A previously delivered head is not lost between registration and waiting.
  await client.waitForHead(100);
  const closing = client.waitForHead(200);
  const rejected = assert.rejects(closing, /Closed|closed/);
  await client.close();
  await rejected;
});
