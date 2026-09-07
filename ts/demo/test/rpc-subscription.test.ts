import { test } from "node:test";
import assert from "node:assert/strict";
import { subscribeRpc } from "../src/rpc-subscription.ts";

test("silent ACK reconnects; old callbacks and foreign subscription IDs cannot affect the new connection", (t) => {
  const sockets: FakeSocket[] = [];
  class FakeSocket {
    onopen: (() => void) | undefined;
    onclose: (() => void) | undefined;
    onerror: (() => void) | undefined;
    onmessage: ((event: { data: string }) => void) | undefined;
    closed = false;
    constructor() { sockets.push(this); }
    send() {}
    close() { this.closed = true; }
    message(data: unknown) { this.onmessage?.({ data: JSON.stringify(data) }); }
  }
  const original = globalThis.WebSocket;
  globalThis.WebSocket = FakeSocket as unknown as typeof WebSocket;
  t.after(() => { globalThis.WebSocket = original; });
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const seen: number[] = [];
  const stop = subscribeRpc({ url: "ws://fixture", method: "test", params: {}, handshakeMs: 100,
    onEvent: (event) => seen.push(event.params.result), onError: (error) => assert.fail(error.message) });
  t.after(stop);
  const first = sockets[0]!;
  first.onopen?.();
  const stale = first.onmessage!;
  t.mock.timers.tick(100);
  assert(first.closed);
  t.mock.timers.tick(400);
  const second = sockets[1]!;
  second.onopen?.();
  second.message({ id: 1, result: "current" });
  stale({ data: JSON.stringify({ id: 1, error: { message: "late error" } }) });
  second.message({ method: "event", params: { subscription_id: "old", result: 1 } });
  second.message({ method: "event", params: { subscription_id: "current", result: 2 } });
  assert.deepEqual(seen, [2]);
  assert.equal(second.closed, false);
  stop();
  t.mock.timers.tick(100_000);
  assert.equal(sockets.length, 2);
});
