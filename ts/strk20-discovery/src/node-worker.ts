import { parentPort, workerData } from "node:worker_threads";
import { readFile } from "node:fs/promises";
import init, { Engine } from "./wasm/strk20_engine.js";
import { installWorker, type WorkerHost } from "./worker.ts";
import { FileCache } from "./node-cache.ts";

if (!parentPort) throw new Error("Discovery must run in a worker thread");
const port = parentPort;
const host: WorkerHost = {
  onmessage: null,
  postMessage: (value) => port.postMessage(value),
};
installWorker(
  async () => {
    await init({
      module_or_path: await readFile(
        new URL("./wasm/strk20_engine_bg.wasm", import.meta.url),
      ),
    });
    return { Engine };
  },
  host,
  (identity) =>
    new FileCache(
      (workerData as { cacheDirectory: string }).cacheDirectory,
      identity,
    ),
);
port.on("message", (data: unknown) => host.onmessage?.({ data }));
