import { Worker } from "node:worker_threads";
import { resolve } from "node:path";
import { LocalDiscoveryProvider } from "./provider.ts";
import type { ClientOptions, WorkerPort } from "./client.ts";

export interface NodeOptions extends Omit<ClientOptions, "workerFactory"> {
  /** Trusted local directory. Contains private discovery results and witnesses. */
  cacheDirectory: string;
}

/** The same provider and WASM runtime, hosted in a Node worker thread. */
export class NodeDiscoveryProvider extends LocalDiscoveryProvider {
  constructor({ cacheDirectory, ...options }: NodeOptions) {
    super({
      ...options,
      workerFactory: () => {
        const worker = new Worker(
          new URL("./node-worker.js", import.meta.url),
          {
            workerData: { cacheDirectory: resolve(cacheDirectory) },
          },
        );
        let stopped = false;
        const port: WorkerPort = {
          onmessage: null,
          onerror: null,
          postMessage: (value) => worker.postMessage(value),
          terminate: () => {
            stopped = true;
            void worker.terminate();
          },
        };
        worker.on("message", (data: unknown) =>
          port.onmessage?.(new MessageEvent("message", { data })),
        );
        const failed = (error: Error) =>
          port.onerror?.(
            Object.assign(new Event("error"), {
              message: error.message,
              error,
              filename: "",
              lineno: 0,
              colno: 0,
            }),
          );
        worker.on("error", failed);
        worker.on("exit", (code) => {
          if (!stopped) failed(new Error(`Worker exited: ${code}`));
        });
        return port;
      },
    });
  }
}
