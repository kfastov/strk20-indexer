import { test } from "node:test";
import assert from "node:assert/strict";
import { feedPath, PublicTransport } from "../src/net.ts";

test("discovery fetches only public artifacts, without per-wallet URL parameters", () => {
  for (const path of [
    "genesis.json",
    "manifest.json",
    "head.ndjson",
    "live",
    "epochs/00000001.strk20e.zst",
    "snapshots/00000001.strk20s.zst",
  ])
    assert(feedPath(path), path);
  for (const path of [
    "../manifest.json",
    "manifest.json?key=1",
    "live?address=0x1",
    "snapshots/latest.strk20s.zst",
    "https://elsewhere/manifest.json",
    "v1/raw/0x1",
    "epochs/1.strk20e.zst",
    "snapshots/00000001.anchor.json",
  ])
    assert(!feedPath(path), path);
  assert.throws(
    () => new PublicTransport("https://feed.test/?key=1", () => {}),
    /CONFIG_INVALID/,
  );
});

test("SSE handles split UTF-8 and CRLF while delivering full data in stream order", async (t) => {
  const payload =
    'event: epoch\r\ndata: {"payload":"ü"}\r\n\r\nevent: head\r\ndata: {"payload":"head"}\r\n\r\n';
  const bytes = new TextEncoder().encode(payload);
  t.mock.method(
    globalThis,
    "fetch",
    async () =>
      new Response(
        new ReadableStream({
          start(controller) {
            for (const byte of bytes) controller.enqueue(Uint8Array.of(byte));
            controller.close();
          },
        }),
        { headers: { "content-type": "text/event-stream" } },
      ),
  );
  const events: string[] = [];
  await new PublicTransport("https://feed.test", () => {}).live(
    (name, data) => events.push(`${name}:${data}`),
    new AbortController().signal,
  );
  assert.deepEqual(events, [
    'epoch:{"payload":"ü"}',
    'head:{"payload":"head"}',
  ]);
});
