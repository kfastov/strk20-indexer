import type {
  EngineInfo,
  RuntimeOptions,
  WorkerEvent,
  ChainProfile,
  DiscoveryResult,
  ChannelResult,
} from "./types.ts";
import { resolveProfile } from "./profiles.ts";

export interface WorkerPort {
  onmessage: ((event: MessageEvent) => void) | null;
  onerror: ((event: ErrorEvent) => void) | null;
  postMessage(value: unknown): void;
  terminate(): void;
}
export interface ClientOptions {
  network?: "mainnet" | "sepolia" | ChainProfile;
  feedUrl: string;
  rpcUrl?: string;
  proofRpcUrl?: string;
  proofSource?: "feed" | "rpc";
  workerFactory?: () => WorkerPort;
  onEvent?: (event: WorkerEvent) => void;
}
export class KeylessClient {
  readonly ready: Promise<EngineInfo>;
  private readonly worker: WorkerPort;
  private failure: Error | undefined;
  private sequence = 0;
  private closed = false;
  private feedHead = -1;
  private headWaiters = new Set<(head: number | Error) => void>();
  private pending = new Map<
    number,
    { resolve: (value: unknown) => void; reject: (reason: Error) => void }
  >();
  constructor(options: ClientOptions) {
    const network = options.network ?? "mainnet";
    const profile = resolveProfile(network);
    this.worker =
      options.workerFactory?.() ??
      new Worker(new URL("./worker-entry.js", import.meta.url), {
        type: "module",
      });
    this.worker.onmessage = (event) => {
      const message = event.data;
      if ("event" in message) {
        if (message.event === "head" && Number.isSafeInteger(message.value) && message.value >= 0) {
          this.feedHead = message.value;
          for (const notify of this.headWaiters) notify(this.feedHead);
        }
        if (message.event === "error" && /CHAIN_MISMATCH|CHECKPOINT_FAILED/.test(message.value))
          for (const notify of this.headWaiters) notify(new Error(message.value));
        options.onEvent?.(message as WorkerEvent);
        return;
      }
      const waiter = this.pending.get(message.id);
      if (!waiter) return;
      this.pending.delete(message.id);
      if (message.error) waiter.reject(new Error(message.error));
      else waiter.resolve(message.result);
    };
    this.worker.onerror = () => {
      this.failure = new Error("Discovery worker failed");
      for (const notify of this.headWaiters) notify(this.failure);
      for (const waiter of this.pending.values()) waiter.reject(this.failure);
      this.pending.clear();
    };
    const config: RuntimeOptions = {
      network,
      feedUrl: options.feedUrl,
      proofSource: options.proofSource ?? "feed",
      rpcUrl:
        options.rpcUrl ??
        (profile.name === "sepolia"
          ? "https://starknet-sepolia-rpc.publicnode.com"
          : "https://starknet.publicnode.com"),
      proofRpcUrl:
        options.proofRpcUrl ??
        (profile.name === "sepolia"
          ? "https://api.cartridge.gg/x/starknet/sepolia"
          : "https://api.cartridge.gg/x/starknet/mainnet"),
    };
    this.ready = this.call("init", [config]) as Promise<EngineInfo>;
  }
  private call(method: string, args: unknown[] = []): Promise<unknown> {
    if (this.failure) return Promise.reject(this.failure);
    if (this.closed)
      return Promise.reject(new Error("Discovery provider is closed"));
    const id = ++this.sequence;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      try {
        this.worker.postMessage({ id, method, args });
      } catch (error) {
        this.pending.delete(id);
        reject(error);
      }
    });
  }
  async discover(
    address: bigint,
    key: bigint,
    block?: number,
    cached = false,
  ): Promise<DiscoveryResult | undefined> {
    await this.ready;
    return this.call("discover", [
      `0x${address.toString(16)}`,
      keyBytes(key),
      block,
      cached,
    ]) as Promise<DiscoveryResult | undefined>;
  }
  async channels(
    address: bigint,
    key: bigint,
    recipients: bigint[] | null,
    block?: number,
  ): Promise<ChannelResult> {
    await this.ready;
    return this.call("channels", [
      `0x${address.toString(16)}`,
      keyBytes(key),
      recipients?.map((n) => `0x${n.toString(16)}`) ?? null,
      block,
    ]) as Promise<ChannelResult>;
  }
  async requirement(
    address: bigint,
    key: bigint,
    recipient: bigint,
    token: bigint,
    block?: number,
  ): Promise<number> {
    await this.ready;
    return this.call("requirement", [
      `0x${address.toString(16)}`,
      keyBytes(key),
      `0x${recipient.toString(16)}`,
      `0x${token.toString(16)}`,
      block,
    ]) as Promise<number>;
  }
  async sync(block?: number): Promise<EngineInfo> {
    await this.ready;
    return this.call("sync", [block]) as Promise<EngineInfo>;
  }
  async subscribe(): Promise<void> {
    await this.ready;
    await this.call("subscribe");
  }
  /** Wait for advertised feed availability, not verification. Discovery still
   * checks the complete state against an independently fetched checkpoint. */
  async waitForHead(block: number): Promise<void> {
    await this.subscribe();
    if (this.feedHead >= block) return;
    if (this.failure) throw this.failure;
    if (this.closed) throw new Error("Discovery provider is closed");
    await new Promise<void>((resolve, reject) => {
      const notify = (head: number | Error) => {
        if (typeof head === "number" && head < block) return;
        clearTimeout(timer);
        this.headWaiters.delete(notify);
        if (head instanceof Error) reject(head);
        else resolve();
      };
      const timer = setTimeout(() => notify(new Error(
        "BOUND_UNAVAILABLE: timed out waiting for feed subscription",
      )), 120_000);
      this.headWaiters.add(notify);
    });
  }
  async clearCache(): Promise<void> {
    await this.ready;
    await this.call("clear");
  }
  async close(): Promise<void> {
    if (this.closed) return;
    try {
      if (!this.failure) await this.call("close");
    } finally {
      this.closed = true;
      for (const notify of this.headWaiters) notify(new Error("Closed"));
      this.worker.terminate();
      for (const w of this.pending.values()) w.reject(new Error("Closed"));
      this.pending.clear();
    }
  }
}
function keyBytes(key: bigint): Uint8Array {
  if (key <= 0n || key >= 2n ** 251n) throw new Error("Invalid viewing key");
  return Uint8Array.from(
    key.toString(16).padStart(64, "0").match(/../g)!,
    (byte) => parseInt(byte, 16),
  );
}
