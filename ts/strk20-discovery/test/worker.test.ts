import { Signer, constants } from "starknet";
import {
  createPrivateTransfers,
  ProvingServiceProofProvider,
} from "@starkware-libs/starknet-privacy-sdk";
import {
  MockProofInvocationFactory,
  compute_note_id,
} from "@starkware-libs/starknet-privacy-sdk/testing";
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createHash } from "node:crypto";
import "fake-indexeddb/auto";
import init, { Engine } from "../../../crates/wasm/pkg/strk20_engine.js";
import { Witness, AddressMap } from "@starkware-libs/starknet-privacy-sdk";
import { WorkerRuntime, installWorker, type WorkerHost } from "../src/worker.ts";
import { notesResult, LocalDiscoveryProvider } from "../src/provider.ts";
import type { DiscoveryResult, RuntimeOptions } from "../src/types.ts";

const file = (name: string) =>
  new Uint8Array(
    readFileSync(
      new URL(`../../../crates/wasm/fixture/${name}`, import.meta.url),
    ),
  );
const doc = (name: string) => JSON.parse(new TextDecoder().decode(file(name)));
await init({
  module_or_path: readFileSync(
    new URL("../../../crates/wasm/pkg/strk20_engine_bg.wasm", import.meta.url),
  ),
});
const genesis = doc("genesis.json"),
  checkpoint = doc("checkpoint.json"),
  owners = doc("owners.json") as { name: string; owner: string; key: string }[];
const options: RuntimeOptions = {
  network: {
    name: "fixture",
    chainId: genesis.chain_id,
    pool: genesis.pool,
    genesisBlock: genesis.genesis_block,
    epochSize: genesis.epoch_size,
    feedFormat: 1,
  },
  feedUrl: "https://feed.test",
  rpcUrl: "https://header.test",
  proofRpcUrl: "https://proof.test",
};
const key = (owner: (typeof owners)[number]) =>
  new Uint8Array(Buffer.from(owner.key, "hex"));

test("real WASM Worker: snapshot/epochs, SDK Witness, cache-only restore and checkpoint failure", async (t) => {
  let manifest = doc("manifest.json"),
    badProof = false;
  let unavailableProofAt: number | undefined;
  let headerGate: { block: number; entered: () => void; resume: Promise<void> } | undefined;
  const requests: { url: string; body: string }[] = [];
  let stream: ReadableStreamDefaultController<Uint8Array> | undefined;
  let connected: (() => void) | undefined;
  let head = file("head.ndjson");
  t.mock.method(
    globalThis,
    "fetch",
    async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input),
        body = String(init?.body ?? "");
      requests.push({ url, body });
      if (body) {
        const call = JSON.parse(body);
        if (headerGate && call.method === "starknet_getBlockWithTxHashes"
          && call.params[0].block_number === headerGate.block) {
          headerGate.entered();
          await headerGate.resume;
        }
        let result: unknown;
        if (call.method === "starknet_chainId")
          result = `0x${Buffer.from(genesis.chain_id).toString("hex")}`;
        else if (call.method === "starknet_getBlockWithTxHashes")
          result = {
            block_number: call.params[0].block_number,
            block_hash: `0x${(0xb10c0000 + call.params[0].block_number).toString(16)}`,
            new_root: checkpoint.state_root,
            status: "ACCEPTED_ON_L2",
          };
        else if (call.method === "starknet_getStorageProof") {
          if (call.params[0].block_number === unavailableProofAt)
            return Response.json({ jsonrpc: "2.0", id: 1,
              error: { code: 24, message: "Block not found" } });
          result = doc("proof.json");
          (
            result as { global_roots: { block_hash: string } }
          ).global_roots.block_hash = `0x${(0xb10c0000 + call.params[0].block_number).toString(16)}`;
          if (badProof)
            (
              result as { global_roots: { block_hash: string } }
            ).global_roots.block_hash = "0x123";
        } else throw new Error(`unexpected RPC ${call.method}`);
        return Response.json({ jsonrpc: "2.0", id: 1, result });
      }
      const path = new URL(url).pathname.slice(1);
      if (path === "live")
        return new Response(
          new ReadableStream({
            start(controller) {
              stream = controller;
              init?.signal?.addEventListener(
                "abort",
                () => controller.close(),
                { once: true },
              );
              connected?.();
            },
          }),
          { headers: { "content-type": "text/event-stream" } },
        );
      if (path === "genesis.json") return Response.json(genesis);
      if (path === "manifest.json") return Response.json(manifest);
      if (path === "head.ndjson")
        return new Response(head, {
          headers: { etag: "fixture-head" },
        });
      if (path === manifest.snapshot?.file)
        return new Response(file("snapshots/0.zst"));
      if (path === "epochs/00000000.strk20e.zst")
        return new Response(file("epochs/0.zst"));
      throw new Error(`unexpected GET ${path}`);
    },
  );
  for (const mode of ["snapshot", "epochs"]) {
    const tasks: (() => Promise<void>)[] = [];
    const runtime = new WorkerRuntime(
      { Engine },
      options,
      () => {},
      (task) => tasks.push(task),
    );
    await runtime.init();
    await runtime.clear();
    manifest = doc("manifest.json");
    if (mode === "epochs") manifest.snapshot = null;
    const state = await runtime.sync();
    assert.equal(state.verifiedAt, 99);
    for (const owner of owners) {
      const buffer = key(owner);
      const result = (await runtime.discover(
        owner.owner,
        buffer,
        99,
      )) as DiscoveryResult;
      assert(buffer.every((b) => b === 0));
      assert.deepEqual(
        result,
        doc(
          `golden/${mode === "epochs" ? "epochs" : "auto"}/${owner.name}-sdk.json`,
        ),
      );
      const sdk = notesResult(result);
      assert(sdk.notes instanceof AddressMap);
      assert.equal(sdk.timestamp, 99);
      for (const [token, notes] of sdk.notes) {
        for (const note of notes) {
          assert(note.witness instanceof Witness);
          assert(note.amount > 0n);
          assert.equal(note.created, 99);
          assert(
            sdk.cursor.incomingChannels
              .get(BigInt(result.notes[0]!.sender))
              ?.noteIndexes.has(token),
          );
        }
      }
      if (owner.name === "alice") {
        const channels = runtime.channels(owner.owner, key(owner), null) as {
          total: number;
          channels: { tokens: { noteNonce: number }[] }[];
        };
        assert.equal(channels.total, 2);
        assert(
          channels.channels.every(
            (channel) => channel.tokens[0]?.noteNonce === 1,
          ),
        );
        assert.equal(
          runtime.requirement(
            owner.owner,
            key(owner),
            owner.owner,
            result.notes[0]!.token,
          ),
          3,
        );
        assert.equal(
          runtime.requirement(
            owner.owner,
            key(owner),
            "0x987",
            result.notes[0]!.token,
          ),
          1,
        );
      }
    }
    await runtime.save();
    const restored = new WorkerRuntime(
      { Engine },
      options,
      () => {},
      (task) => tasks.push(task),
    );
    requests.length = 0;
    assert.equal((await restored.init()).verifiedAt, 99);
    const again = await restored.discover(
      owners[0]!.owner,
      key(owners[0]!),
      undefined,
      true,
    );
    assert(again);
    assert.equal(
      requests.length,
      0,
      "warm restore performs no network or epoch replay",
    );
    badProof = true;
    await assert.rejects(() => restored.sync(), /block hash|block_hash|proof/i);
    await assert.rejects(
      () =>
        restored.discover(owners[0]!.owner, key(owners[0]!), undefined, true),
      /CHECKPOINT_FAILED/,
    );
    badProof = false;
    await restored.close();
    await runtime.close();
  }
  // The public SDK adapter and actual dispatcher; only signing/proving is replaced.
  const scope: WorkerHost = {
    postMessage: () => {},
    onmessage: null,
  };
  const port = {
    onmessage: null as ((event: MessageEvent) => void) | null,
    onerror: null,
    postMessage: (data: unknown) => scope.onmessage?.({ data }),
    terminate: () => {},
  };
  scope.postMessage = (data) =>
    queueMicrotask(() => port.onmessage?.({ data } as MessageEvent));
  installWorker(async () => ({ Engine }), scope);
  let headObserved: (() => void) | undefined;
  const provider = new LocalDiscoveryProvider({
    ...options,
    workerFactory: () => port as unknown as Worker,
    onEvent: (e) => { if (e.event === "head") headObserved?.(); },
  });
  const owner = owners[0]!,
    viewingKey = BigInt(`0x${owner.key}`);
  const mine = provider.forAccount({ address: owner.owner, viewingKey });
  const found = await mine.discoverNotes({ blockIdentifier: 99 });
  const [token, items] = [...found.notes][0]!;
  const note = items[0]!;
  assert.equal(
    BigInt(note.id),
    compute_note_id(note.witness.channelKey, token, note.witness.nonce),
  );
  const transfers = createPrivateTransfers({
    account: { address: owner.owner, signer: new Signer("0x1") },
    viewingKeyProvider: { getViewingKey: async () => viewingKey },
    poolContractAddress: genesis.pool,
    discoveryProvider: provider.atBlock(99),
    provingProvider: new ProvingServiceProofProvider(
      "https://prover.invalid",
      constants.StarknetChainId.SN_SEPOLIA,
    ),
    proofInvocationFactory: new MockProofInvocationFactory(),
  });
  const invocation = await transfers
    .build({
      autoRegister: true,
      autoSetup: true,
      autoDiscover: { notes: "refresh", channels: "refresh" },
      autoSelectNotes: "naive",
    })
    .surplusTo(owner.owner)
    .with(token, (builder) =>
      builder.transfer({ amount: 10n, recipient: owner.owner }),
    )
    .createProofInvocation({ provingBlockId: 99 });
  const actions = JSON.parse(invocation.invocation.calldata[2]!) as {
    type: string;
    input: { channel_key: string; index: number };
  }[];
  const spend = actions.find((action) => action.type === "UseNote");
  assert(spend, "the official builder must spend our discovered note");
  assert.equal(spend.input.channel_key, `__bigint__${note.witness.channelKey}`);
  assert.equal(spend.input.index, note.witness.nonce);

  // A recipient without a registered public key must be absent, not a
  // Channel(publicKey=0): the SDK inserts the self-channel when registering.
  const fresh = 0x123456789abcn;
  const freshChannels = await provider.discoverChannels(fresh, 1n, [fresh], {
    blockIdentifier: 99,
  });
  assert(freshChannels.channels);
  assert(!freshChannels.channels.has(fresh));
  assert.equal(
    await provider.discoverRequirement(fresh, 1n, fresh, token, 99),
    0,
  );
  const freshTransfers = createPrivateTransfers({
    account: { address: `0x${fresh.toString(16)}`, signer: new Signer("0x1") },
    viewingKeyProvider: { getViewingKey: async () => 1n },
    poolContractAddress: genesis.pool,
    discoveryProvider: provider.atBlock(99),
    provingProvider: new ProvingServiceProofProvider(
      "https://prover.invalid",
      constants.StarknetChainId.SN_SEPOLIA,
    ),
    proofInvocationFactory: new MockProofInvocationFactory(),
  });
  const registration = await freshTransfers
    .build({
      autoRegister: true,
      autoSetup: true,
      autoDiscover: { notes: "refresh", channels: "refresh" },
    })
    .surplusTo(fresh)
    .with(token, (builder) => builder.deposit({ amount: 10n }))
    .createProofInvocation({ provingBlockId: 99 });
  const registrationActions = JSON.parse(registration.invocation.calldata[2]!);
  assert.equal(
    registrationActions.filter((a: { type: string }) => a.type === "SetViewingKey")
      .length,
    1,
  );
  // A foreground request arriving during an active verification must precede
  // an already queued SSE refresh, rather than verify an unrelated head first.
  let releaseHeader!: () => void;
  const entered = new Promise<void>((resolve) => {
    headerGate = { block: 98, entered: resolve,
      resume: new Promise<void>((resume) => { releaseHeader = resume; }) };
  });
  try {
    const connection = new Promise<void>((resolve) => { connected = resolve; });
    await provider.subscribe();
    await connection;
    requests.length = 0;
    const first = mine.discoverNotes({ blockIdentifier: 98 });
    await entered;
    const observed = new Promise<void>((resolve) => { headObserved = resolve; });
    stream!.enqueue(new TextEncoder().encode(
      `event: head\ndata: ${JSON.stringify({ head: 99, payload: null, resync: true })}\n\n`,
    ));
    await observed;
    const second = mine.discoverNotes({ blockIdentifier: 97 });
    // Deliver the foreground message while the first verification is paused.
    await new Promise<void>((resolve) => setImmediate(resolve));
    releaseHeader();
    await Promise.all([first, second]);
    const proofBlocks = requests.filter((r) => r.body.includes("starknet_getStorageProof"))
      .map((r) => JSON.parse(r.body).params[0].block_number);
    assert.deepEqual(proofBlocks.slice(0, 2), [98, 97],
      "queued live verification must yield to the waiting foreground read");
  } finally {
    releaseHeader();
    headerGate = undefined;
    await provider.close();
  }

  // An actual epoch advance arrives entirely in SSE; only the independent
  // checkpoint RPCs are allowed before the new state becomes visible.
  const liveTasks: (() => Promise<void>)[] = [];
  const liveSpans: string[] = [];
  let notify: (() => void) | undefined;
  const live = new WorkerRuntime(
    { Engine },
    options,
    (event) => {
      if (event.event === "span") liveSpans.push(event.value.name);
    },
    (task) => {
      liveTasks.push(task);
      notify?.();
    },
  );
  try {
  await live.init();
  await live.sync();
  await assert.rejects(() => live.sync(198), /BOUND_UNAVAILABLE/);
  assert(requests.some((r) => r.body.includes("starknet_getStorageProof")
    && JSON.parse(r.body).params[0].block_number === 198),
    "checkpoint acquisition starts before the requested feed block arrives");
  const connection = new Promise<void>((resolve) => {
    connected = resolve;
  });
  live.subscribe();
  await connection;
  const original = doc("manifest.json");
  const payload = [
    {
      t: "hdr",
      v: 1,
      kind: "strk20-epoch",
      chain_id: genesis.chain_id,
      pool: genesis.pool,
      epoch: 1,
      from: 100,
      to: 199,
      prev: original.epochs[0].hash,
    },
    { t: "end", blocks: 0, diffs: 0, events: 0, class: "0xc1a55" },
  ]
    .map((row) => JSON.stringify(row) + "\n")
    .join("");
  const entry = {
    e: 1,
    from: 100,
    to: 199,
    hash: createHash("sha256").update(payload).digest("hex"),
    zst: "0".repeat(64),
    bytes: 0,
    anchor: null,
  };
  head = new TextEncoder().encode(
    [
      {
        t: "hdr",
        v: 1,
        kind: "strk20-head",
        tail_from: 200,
        head: 199,
        head_hash: "0xb10c00c7",
        l1_accepted: 199,
      },
      { t: "end", blocks: 0, diffs: 0, events: 0, class: "0xc1a55" },
    ]
      .map((row) => JSON.stringify(row) + "\n")
      .join(""),
  );
  const send = (name: string, data: unknown) =>
    stream!.enqueue(
      new TextEncoder().encode(
        `event: ${name}\ndata: ${JSON.stringify(data)}\n\n`,
      ),
    );
  const update = {
    head: 199,
    head_hash: "0xb10c00c7",
    l1_accepted: 199,
    etag: "live-199",
    payload: new TextDecoder().decode(head),
    resync: false,
  };
  requests.length = 0;
  const queued = new Promise<void>((resolve) => {
    notify = resolve;
  });
  send("epoch", { entry, payload });
  send("head", update);
  await queued;
  liveSpans.length = 0;
  liveTasks.push(async () => {
    await live.discover(owners[0]!.owner, key(owners[0]!), 198);
  });
  while (liveTasks.length) await liveTasks.shift()!();
  assert.equal(live.info().verifiedAt, 198, "SSE first serves the waiting foreground bound");
  assert.equal(live.info().last_epoch, 1);
  assert(liveSpans.indexOf("Discover notes") < liveSpans.indexOf("Save verified state"),
    "a read queued behind a live update finishes before persisting the cache");
  const proofRequests = () => requests.filter((r) => r.body.includes("starknet_getStorageProof")).length;
  assert.equal(proofRequests(), 0, "the proof was prefetched while waiting for the feed");
  await live.sync(198);
  assert.equal(proofRequests(), 0, "the foreground retry must not acquire a second proof");
  assert.equal(
    requests.filter((r) => !r.body).length,
    0,
    "inline SSE never re-downloads head or epochs",
  );

  requests.length = 0;
  assert.equal((await live.sync(197)).verifiedAt, 197);
  assert.equal(requests.filter((r) => !r.body).length, 0,
    "bounded reads reuse staged artifacts rather than reload the feed");
  assert(requests.some((r) => r.body.includes("starknet_getStorageProof")),
    "artifact reuse still independently proves the requested checkpoint");
  badProof = true;
  await assert.rejects(() => live.sync(196), /block hash|block_hash|proof/i);
  badProof = false;

  // A bounded resync signal uses the same HTTP apply path.
  manifest = {
    ...original,
    latest_epoch: 1,
    epochs: [...original.epochs, entry],
    head: {
      ...original.head,
      number: 199,
      hash: update.head_hash,
      l1_accepted: 199,
    },
  };
  requests.length = 0;
  const resync = new Promise<void>((resolve) => {
    notify = resolve;
  });
  send("head", { ...update, payload: null, resync: true });
  await resync;
  while (liveTasks.length) await liveTasks.shift()!();
  assert.equal(live.info().verifiedAt, 199);
  assert(requests.some((r) => r.url.endsWith("/manifest.json")));
  assert(requests.some((r) => r.url.endsWith("/head.ndjson")));
  // A foreground bound whose proof remains unavailable must not pin all
  // subsequent background updates to that failed checkpoint forever.
  unavailableProofAt = 200;
  await assert.rejects(() => live.sync(200), /BOUND_UNAVAILABLE/);
  const advance = async (height: number) => {
    const hash = `0x${(0xb10c0000 + height).toString(16)}`;
    const rows = new TextDecoder().decode(head).trim().split("\n").map((r) => JSON.parse(r));
    Object.assign(rows[0], { head: height, head_hash: hash });
    const queued = new Promise<void>((resolve) => { notify = resolve; });
    send("head", { ...update, head: height, head_hash: hash, etag: `live-${height}`,
      payload: rows.map((r) => JSON.stringify(r) + "\n").join("") });
    await queued;
    while (liveTasks.length) await liveTasks.shift()!();
  };
  await assert.rejects(() => advance(201), /RPC_UNAVAILABLE: 24/);
  await advance(202);
  assert.equal(live.info().verifiedAt, 202, "an unavailable old proof must not stall the live stream");
  unavailableProofAt = undefined;

  for (const request of requests) {
    if (request.body) {
      const rpc = JSON.parse(request.body);
      assert(
        [
          "starknet_chainId",
          "starknet_getBlockWithTxHashes",
          "starknet_getStorageProof",
        ].includes(rpc.method),
      );
      assert(!request.body.includes("viewing_key"));
    }
  }
  } finally { unavailableProofAt = undefined; await live.close(); }
});
