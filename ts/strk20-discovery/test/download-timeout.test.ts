import { test } from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { PublicTransport } from "../src/net.ts";

test("artifact downloads allow steady progress, but bound stalls, total time and cancellation", async (t) => {
  // Accelerate only the SDK's deadlines. Body reads and aborts use real HTTP.
  const timeout = globalThis.setTimeout;
  const timeoutSignal = AbortSignal.timeout;
  t.mock.method(AbortSignal, "timeout", (ms: number) => timeoutSignal(ms === 30_000 ? 1_000 : ms));
  t.mock.method(globalThis, "setTimeout", ((callback: (...args: any[]) => void, ms?: number, ...args: any[]) =>
    timeout(callback, ms === 30_000 ? 1_000 : ms === 300_000 ? 3_000 : ms, ...args)) as typeof setTimeout);
  let mode: "progress" | "stalled" | "endless" = "progress";
  let entered: (() => void) | undefined;
  const server = createServer((_request, response) => {
    response.writeHead(200, { "content-type": "application/octet-stream" });
    response.write("a");
    entered?.();
    if (mode === "stalled") return;
    let chunks = 1;
    const timer = setInterval(() => {
      response.write("b");
      if (++chunks === 5 && mode === "progress") response.end();
    }, 400);
    response.once("close", () => clearInterval(timer));
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  t.after(async () => {
    server.closeAllConnections();
    await new Promise<void>((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
  });
  const address = server.address() as { port: number };
  const net = new PublicTransport(`http://127.0.0.1:${address.port}`, () => {});
  const result = await net.get("snapshots/00000000.strk20s.zst");
  assert.equal(new TextDecoder().decode(result.bytes), "abbbb",
    "a download longer than the idle deadline succeeds while bytes arrive");
  await assert.rejects(net.get("manifest.json"), /timeout/i,
    "small metadata requests retain their original total deadline");

  mode = "stalled";
  await assert.rejects(net.get("epochs/00000000.strk20e.zst"), /download idle for 30s.*\(1 bytes received\)/);

  mode = "endless";
  await assert.rejects(net.get("snapshots/00000000.strk20s.zst"), /download exceeded 5 minutes/);

  mode = "stalled";
  const control = new AbortController();
  const opened = new Promise<void>((resolve) => { entered = resolve; });
  const cancelled = net.get("snapshots/00000000.strk20s.zst", control.signal);
  const rejection = assert.rejects(cancelled, /cancelled by caller/);
  await opened;
  control.abort(new Error("cancelled by caller"));
  await rejection;
});
