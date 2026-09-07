import { inflateWithin } from "./decompress.ts";
import { CheckpointSource } from "./checkpoint.ts";
import type { Engine, EngineModule } from "./engine.ts";
import type {
  RuntimeOptions,
  EngineInfo,
  Manifest,
  EpochEntry,
  WorkerEvent,
  StartupProgress,
} from "./types.ts";
import { resolveProfile } from "./profiles.ts";
import { PublicTransport } from "./net.ts";
import { StateCache, type CacheStore, type CacheFactory } from "./storage.ts";

const decode = (bytes: Uint8Array) => new TextDecoder().decode(bytes);
const encode = (text: string) => new TextEncoder().encode(text);
const digest = async (bytes: Uint8Array) =>
  Array.from(
    new Uint8Array(
      await crypto.subtle.digest("SHA-256", new Uint8Array(bytes)),
    ),
    (b) => b.toString(16).padStart(2, "0"),
  ).join("");
const hex = (value: string) => `0x${BigInt(value).toString(16)}`;
/** Worker-only host. All expensive state, network and IndexedDB operations stay here. */
export class WorkerRuntime {
  private engine!: Engine;
  private manifest: Manifest | undefined;
  private readonly profile;
  private readonly net: PublicTransport;
  private readonly cache: CacheStore;
  private readonly genesis: string;
  private liveAbort: AbortController | undefined;
  private queuedHead:
    | {
        payload: string;
        etag: string;
        head: number;
        head_hash: string;
        l1_accepted: number;
        resync: boolean;
      }
    | undefined;
  private queuedEpochs = new Map<
    number,
    { entry: EpochEntry; payload: string }
  >();
  private liveQueued = false;
  private needsCatchup = true;
  private headStaged = false;
  private requestedBlock: number | undefined;
  private savedRevision = -1;
  private writing: Promise<void> | undefined;
  private saveTimer: ReturnType<typeof setTimeout> | undefined;
  private clearing = false;
  private genesisChecked = false;
  private stopped = false;
  private initializing = false;
  private readonly module: EngineModule;
  private readonly checkpoints: CheckpointSource;
  private readonly emit: (event: WorkerEvent) => void;
  private readonly enqueue: (job: () => Promise<void>) => void;
  constructor(
    module: EngineModule,
    opts: RuntimeOptions,
    emit: (event: WorkerEvent) => void,
    enqueue: (job: () => Promise<void>) => void,
    cacheFactory: CacheFactory = (name) => new StateCache(name),
  ) {
    this.module = module;
    this.emit = emit;
    this.enqueue = enqueue;
    this.profile = resolveProfile(opts.network);
    this.genesis = JSON.stringify({
      format: "strk20-feed",
      v: 1,
      chain_id: this.profile.chainId,
      pool: hex(this.profile.pool),
      genesis_block: this.profile.genesisBlock,
      epoch_size: this.profile.epochSize,
    });
    this.net = new PublicTransport(opts.feedUrl, (value) =>
      emit({ event: "request", value }),
    );
    this.checkpoints = new CheckpointSource(this.net, opts, this.profile);
    this.cache = cacheFactory(
      `strk20-folded-v2:${this.profile.chainId}:${hex(this.profile.pool)}`,
    );
  }
  private async span<T>(name: string, work: () => T | Promise<T>, bytes?: number): Promise<T> {
    const start = performance.now();
    try {
      return await work();
    } finally {
      this.emit({
        event: "span",
        value: { name, ms: performance.now() - start, ...(bytes === undefined ? {} : { bytes }) },
      });
    }
  }
  info(): EngineInfo {
    return JSON.parse(this.engine.info()) as EngineInfo;
  }
  private startup(value: StartupProgress): void {
    if (this.initializing) this.emit({ event: "startup", value });
  }
  async init(): Promise<EngineInfo> {
    this.initializing = true;
    try {
      this.startup({ stage: "cache" });
      const bytes = await this.span("Read local cache", () => this.cache.read());
      if (bytes) {
        try {
          this.engine = await this.span("Restore verified state", () =>
            this.module.Engine.load(bytes, this.genesis), bytes.length,
          );
          this.savedRevision = this.engine.cache_revision();
        } catch {
          await this.cache.clear();
        }
      }
      this.engine ??= new this.module.Engine(this.genesis);
      if (this.info().verifiedAt === null || this.info().verificationFailed) {
        await this.sync();
        this.startup({ stage: "save" });
        await this.save();
      }
      const state = this.info();
      this.startup({ stage: "ready" });
      return state;
    } finally {
      this.initializing = false;
    }
  }
  async save(): Promise<void> {
    while (this.writing) await this.writing;
    if (this.savedRevision === this.engine.cache_revision() || this.info().verifiedAt === null) return;
    const revision = this.engine.cache_revision();
    const work = this.span("Save verified state", async () => {
      const bytes = await this.span("Serialize verified state", () => this.engine.export_state());
      await this.cache.write(bytes);
      this.savedRevision = revision;
    });
    this.writing = work;
    try { await work; }
    finally { if (this.writing === work) this.writing = undefined; }
  }
  private scheduleSave(): void {
    if (this.stopped || this.clearing || this.saveTimer !== undefined
      || this.savedRevision === this.engine.cache_revision()) return;
    // Yield to incoming Worker messages. Disk I/O is independent of the command
    // queue; one writer and a revision keep an old completion from marking new
    // state as saved. No timer is armed for unchanged state.
    this.saveTimer = setTimeout(() => {
      this.saveTimer = undefined;
      void this.save().then(() => this.scheduleSave(), (error) =>
        this.emit({ event: "error", value: String(error) }));
    }, 0);
  }
  canReadNow(method: string, args: unknown[]): boolean {
    if (!this.engine || this.stopped || this.clearing) return false;
    const info = this.info();
    if (info.verificationFailed || info.verifiedAt === null) return false;
    if (method === "discover" && args[3] === true) return true;
    const block = args[method === "discover" ? 2 : method === "channels" ? 3 : 4];
    return ["discover", "channels", "requirement"].includes(method) && block === info.verifiedAt;
  }
  readNow(method: string, args: unknown[]): unknown {
    // Keep the eligibility check and the read in one synchronous turn: an
    // awaiting sync must not apply a different block between the two.
    if (!this.canReadNow(method, args)) throw new Error("Verified state changed before read");
    const owner = args[0] as string, key = args[1] as Uint8Array;
    if (method === "requirement")
      return this.requirement(owner, key, args[2] as string, args[3] as string);
    const start = performance.now();
    let notes: unknown;
    try { notes = JSON.parse(this.engine.discover(owner, method === "channels" ? key.slice() : key)); }
    finally { this.emit({ event: "span", value: { name: "Discover notes", ms: performance.now() - start } }); }
    this.scheduleSave();
    return method === "channels" ? this.channels(owner, key, args[2] as string[] | null) : notes;
  }
  private async manifestGet(): Promise<void> {
    const [g, m] = await Promise.all([
      this.genesisChecked ? undefined : this.net.get("genesis.json"),
      this.net.get("manifest.json"),
    ]);
    const genesis = g ? JSON.parse(decode(g.bytes)) as {
      chain_id: string;
      pool: string;
      genesis_block: number;
      epoch_size: number;
    } : JSON.parse(this.genesis);
    if (
      genesis.chain_id !== this.profile.chainId ||
      hex(genesis.pool) !== hex(this.profile.pool) ||
      genesis.genesis_block !== this.profile.genesisBlock ||
      genesis.epoch_size !== this.profile.epochSize
    )
      throw new Error("CHAIN_MISMATCH: feed genesis");
    this.genesisChecked = true;
    this.manifest = JSON.parse(decode(m.bytes)) as Manifest;
    this.needsCatchup = false;
  }
  async sync(block?: number, retry = 0): Promise<EngineInfo> {
    const previous = this.info();
    if (
      block !== undefined &&
      previous.verifiedAt === block &&
      !previous.verificationFailed
    )
      return previous;
    // If a foreground read is waiting for B, let the arriving SSE update
    // verify B first. Otherwise it verifies H and the queued read immediately
    // downloads another proof for B. This is a single coalesced scheduling hint.
    const target = block ?? (this.requestedBlock !== undefined
      && this.queuedHead && this.queuedHead.head >= this.requestedBlock
      ? this.requestedBlock : undefined);
    const prepare = target ?? this.queuedHead?.head;
    if (prepare !== undefined && Number.isSafeInteger(prepare) && prepare >= 0)
      this.checkpoints.prepare(prepare);
    try {
      const state = await this.syncOnce(target);
      if (block !== undefined || state.verifiedAt === this.requestedBlock)
        this.requestedBlock = undefined;
      return state;
    } catch (error) {
      // This is a scheduling hint, not a permanent bound on the live stream.
      // Once a covering feed arrives, a failed proof must not pin later updates.
      if (target === this.requestedBlock && !String(error).includes("BOUND_UNAVAILABLE"))
        this.requestedBlock = undefined;
      if (block !== undefined && Number.isSafeInteger(block) && block >= 0
        && String(error).includes("BOUND_UNAVAILABLE: block not present"))
        this.requestedBlock = block;
      if (
        retry === 0 &&
        /FEED_ADVANCED_MIDSYNC|FEED_EPOCH_GAP/.test(String(error))
      ) {
        this.needsCatchup = true;
        this.queuedHead = undefined;
        this.queuedEpochs.clear();
        return this.sync(block, 1);
      }
      throw error;
    }
  }
  private async syncOnce(block?: number): Promise<EngineInfo> {
    this.startup({ stage: "feed" });
    // A bounded read may reuse staged public artifacts. The independently
    // fetched checkpoint still verifies the requested block, including reorgs.
    const reuse = block !== undefined && this.headStaged && !this.needsCatchup
      && !this.queuedHead && this.manifest && block <= this.manifest.head.number;
    if (!reuse && (this.needsCatchup || !this.manifest || !this.queuedHead))
      await this.span("Fetch feed manifest", () => this.manifestGet());
    const m = this.manifest!;
    const pending = this.queuedHead;
    this.queuedHead = undefined;
    const stagedEpochs = new Set<number>();
    if (pending && !pending.resync && pending.payload) {
      for (const [e, update] of [...this.queuedEpochs].sort(
        ([a], [b]) => a - b,
      )) {
        const existing = m.epochs.find((row) => row.e === e);
        if (existing?.hash === update.entry.hash) {
          this.engine.stage_epoch(BigInt(e), encode(update.payload));
          stagedEpochs.add(e);
          continue;
        }
        if (e !== (m.latest_epoch ?? -1) + 1) {
          this.needsCatchup = true;
          break;
        }
        m.epochs.push(update.entry);
        m.latest_epoch = e;
        this.engine.stage_epoch(BigInt(e), encode(update.payload));
        stagedEpochs.add(e);
      }
      this.queuedEpochs.clear();
      if (this.needsCatchup)
        throw new Error("FEED_EPOCH_GAP: live stream missed epochs");
      m.head = {
        ...m.head,
        number: pending.head,
        hash: pending.head_hash,
        l1_accepted: pending.l1_accepted,
      };
      this.engine.stage_head(encode(pending.payload), pending.etag);
    } else if (!reuse) {
      const head = await this.net.get("head.ndjson");
      this.engine.stage_head(
        head.bytes,
        head.etag || (await digest(head.bytes)),
      );
    }
    this.headStaged = true;
    if (
      block !== undefined &&
      (!Number.isSafeInteger(block) || block < 0 || block > m.head.number)
    )
      throw new Error("BOUND_UNAVAILABLE: block not present in feed");
    this.startup({ stage: "checkpoint" });
    const cp = await this.span("Wait for checkpoint proof", () =>
      this.checkpoints.acquire(block ?? m.head.number),
    );
    this.engine.stage_checkpoint(JSON.stringify(cp.checkpoint), cp.proof);
    const info = this.info();
    const from = info.last_epoch ?? m.snapshot?.e ?? -1;
    const epochs = m.epochs.filter((e) => e.e > from);
    const total = epochs.length + Number(info.last_epoch === null && !!m.snapshot);
    let completed = 0;
    this.startup({ stage: "download", completed, total });
    if (info.last_epoch === null && m.snapshot) {
      if (m.snapshot.block > cp.checkpoint.block_number)
        throw new Error(
          "BOUND_BELOW_SNAPSHOT: wait for the feed snapshot to mature",
        );
      const snapshot = await this.net.get(m.snapshot.file);
      const payload = await this.span("Inflate snapshot", () =>
        inflateWithin(snapshot.bytes),
      );
      this.engine.stage_snapshot(BigInt(m.snapshot.e), snapshot.bytes, payload);
      this.startup({ stage: "download", completed: ++completed, total });
    }
    for (const entry of epochs) {
      if (stagedEpochs.has(entry.e)) {
        this.startup({ stage: "download", completed: ++completed, total });
        continue;
      }
      const inline = this.queuedEpochs.get(entry.e);
      const payload = inline
        ? encode(inline.payload)
        : inflateWithin(
            (
              await this.net.get(
                `epochs/${String(entry.e).padStart(8, "0")}.strk20e.zst`,
              )
            ).bytes,
          );
      this.engine.stage_epoch(BigInt(entry.e), payload);
      this.startup({ stage: "download", completed: ++completed, total });
    }
    this.engine.stage_manifest(JSON.stringify(m));
    this.startup({ stage: "verify" });
    await this.span("Verify pool state", () => this.engine.apply("auto"));
    this.scheduleSave();
    const state = this.info();
    this.emit({ event: "state", value: state });
    return state;
  }
  async discover(
    owner: string,
    key: Uint8Array,
    block?: number,
    cached = false,
  ): Promise<unknown> {
    try {
      if (!cached) await this.sync(block);
      if (this.info().verifiedAt === null) return undefined;
      const result = await this.span("Discover notes", () =>
        JSON.parse(this.engine.discover(owner, key)),
      );
      this.scheduleSave();
      return result;
    } finally {
      key.fill(0);
    }
  }
  channels(
    owner: string,
    key: Uint8Array,
    recipients: string[] | null,
  ): unknown {
    try {
      return JSON.parse(
        this.engine.channels(owner, key, JSON.stringify(recipients)),
      );
    } finally {
      key.fill(0);
    }
  }
  requirement(
    owner: string,
    key: Uint8Array,
    recipient: string,
    token: string,
  ): number {
    try {
      return this.engine.requirement(owner, key, recipient, token);
    } finally {
      key.fill(0);
    }
  }
  subscribe(): void {
    if (this.liveAbort) return;
    const control = new AbortController();
    this.liveAbort = control;
    void (async () => {
      while (!control.signal.aborted) {
        try {
          await this.net.live((name, text) => {
            const data = JSON.parse(text);
            if (
              name === "hello" &&
              (data.chain_id !== this.profile.chainId ||
                hex(data.pool) !== hex(this.profile.pool))
            )
              throw new Error("CHAIN_MISMATCH: stream");
            if (name === "hello") this.checkpoints.stream(data.proofs === true);
            if (name === "proof") {
              if (data.reset) this.checkpoints.reset();
              else this.checkpoints.receive(data);
            }
            if (name === "status" && data.verify_root_failed)
              throw new Error(
                "CHECKPOINT_FAILED: publisher stopped verification",
              );
            if (name === "epoch" && data.payload && data.entry) {
              this.queuedEpochs.set(data.entry.e, data);
              if (this.queuedEpochs.size > 4) {
                this.queuedEpochs.clear();
                this.needsCatchup = true;
              }
            }
            if (name === "head") {
              this.checkpoints.head(data.head);
              this.queuedHead = data;
              this.emit({ event: "head", value: data.head });
              if (!data.payload || data.resync || this.queuedEpochs.size > 4)
                this.needsCatchup = true;
              if (!this.liveQueued) {
                this.liveQueued = true;
                this.enqueue(async () => {
                  this.liveQueued = false;
                  if (this.stopped) return;
                  await this.sync();
                  this.scheduleSave();
                });
              }
            }
          }, control.signal);
        } catch (e) {
          this.checkpoints.stream(false);
          if (control.signal.aborted) return;
          this.emit({ event: "error", value: String(e) });
          if (/CHAIN_MISMATCH|CHECKPOINT_FAILED/.test(String(e))) {
            control.abort();
            return;
          }
        }
        this.checkpoints.stream(false);
        await new Promise((resolve) => setTimeout(resolve, 1000));
      }
    })();
  }
  async clear(): Promise<void> {
    this.clearing = true;
    clearTimeout(this.saveTimer);
    this.saveTimer = undefined;
    await this.writing?.catch(() => {});
    this.checkpoints.reset();
    await this.cache.clear();
    this.engine.free();
    this.engine = new this.module.Engine(this.genesis);
    this.manifest = undefined;
    this.headStaged = false;
    this.requestedBlock = undefined;
    this.needsCatchup = true;
    this.savedRevision = -1;
    this.clearing = false;
  }
  async close(): Promise<void> {
    if (this.stopped) return;
    this.stopped = true;
    clearTimeout(this.saveTimer);
    this.saveTimer = undefined;
    this.checkpoints.cancel();
    this.liveAbort?.abort();
    if (!this.engine) return; // initialization can fail while opening storage
    try {
      await this.save();
    } finally {
      this.engine.free();
    }
  }
}

/** Bundlers can import this from a custom Worker that statically imports WASM. */
export interface WorkerHost {
  postMessage(value: unknown): void;
  onmessage: ((e: Pick<MessageEvent, "data">) => void) | null;
}
export function installWorker(
  load: () => Promise<EngineModule>,
  scope: WorkerHost = globalThis as unknown as WorkerHost,
  cacheFactory?: CacheFactory,
): void {
  let runtime: WorkerRuntime | undefined;
  const foreground: (() => Promise<void>)[] = [];
  const background: (() => Promise<void>)[] = [];
  let running = false;
  const emit = (event: WorkerEvent) => scope.postMessage(event);
  const enqueue = (work: () => Promise<void>, urgent = false) => {
    (urgent ? foreground : background).push(work);
    if (running) return;
    running = true;
    void (async () => {
      let next;
      // Never interrupt a state mutation. Between jobs, reads take precedence
      // over coalesced live updates and cache writes.
      while ((next = foreground.shift() ?? background.shift())) {
        try { await next(); }
        catch (e) { emit({ event: "error", value: String(e) }); }
      }
      running = false;
    })();
  };
  scope.onmessage = (event) => {
    const { id, method, args } = event.data as {
      id: number;
      method: string;
      args: unknown[];
    };
    const run = async () => {
      try {
        let result: unknown;
        if (method === "init") {
          emit({ event: "startup", value: { stage: "engine" } });
          runtime = new WorkerRuntime(
            await load(),
            args[0] as RuntimeOptions,
            emit,
            enqueue,
            cacheFactory,
          );
          result = await runtime.init();
        } else {
          if (!runtime) throw new Error("Worker is not initialized");
          switch (method) {
            case "sync":
              result = await runtime.sync(args[0] as number | undefined);
              break;
            case "discover":
              result = await runtime.discover(
                args[0] as string,
                args[1] as Uint8Array,
                args[2] as number | undefined,
                args[3] as boolean,
              );
              break;
            case "channels":
              await runtime.discover(
                args[0] as string,
                (args[1] as Uint8Array).slice(),
                args[3] as number | undefined,
              );
              result = runtime.channels(
                args[0] as string,
                args[1] as Uint8Array,
                args[2] as string[] | null,
              );
              break;
            case "requirement":
              await runtime.sync(args[4] as number | undefined);
              result = runtime.requirement(
                args[0] as string,
                args[1] as Uint8Array,
                args[2] as string,
                args[3] as string,
              );
              break;
            case "subscribe":
              runtime.subscribe();
              break;
            case "clear":
              await runtime.clear();
              break;
            case "close":
              await runtime.close();
              break;
            default:
              throw new Error("Unknown worker command");
          }
        }
        scope.postMessage({ id, result });
      } catch (e) {
        scope.postMessage({
          id,
          error: e instanceof Error ? e.message : String(e),
        });
      } finally {
        if (args[1] instanceof Uint8Array) args[1].fill(0);
      }
    };
    // Staged feed/checkpoint bytes do not replace the verified store until
    // synchronous Engine.apply succeeds. Reads of that store can run while a
    // queued sync is waiting for network or a cache write is pending.
    if (runtime?.canReadNow(method, args)) {
      try { scope.postMessage({ id, result: runtime.readNow(method, args) }); }
      catch (error) { scope.postMessage({ id, error: error instanceof Error ? error.message : String(error) }); }
      finally { if (args[1] instanceof Uint8Array) args[1].fill(0); }
    } else enqueue(run, method !== "close");
  };
}
