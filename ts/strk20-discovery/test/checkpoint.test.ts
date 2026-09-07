import { test } from "node:test";
import assert from "node:assert/strict";
import { CheckpointSource } from "../src/checkpoint.ts";
import { PublicTransport } from "../src/net.ts";
import { resolveProfile } from "../src/profiles.ts";

test("a live proof completes an acquisition without proof HTTP or RPC; disconnect catches up once", async (t) => {
  const profile = resolveProfile("sepolia");
  const net = new PublicTransport("https://feed.test", () => {});
  const gets: string[] = [];
  t.mock.method(net, "rpc", async (url: string, method: string, params: any[]) => {
    assert.equal(url, "https://header.test");
    if (method === "starknet_chainId") return `0x${Buffer.from(profile.chainId).toString("hex")}`;
    assert.equal(method, "starknet_getBlockWithTxHashes");
    return { block_number: params[0].block_number, block_hash: "0x123", new_root: "0x456", status: "ACCEPTED_ON_L2" };
  });
  const envelope = (block: number) => ({ block, block_hash: "0x123", proof: { global_roots: { block_hash: "0x123" } } });
  t.mock.method(net, "get", async (path: string) => {
    gets.push(path);
    return { bytes: new TextEncoder().encode(JSON.stringify(envelope(Number(path.split("/")[1])))), etag: "" };
  });
  const source = new CheckpointSource(net, { network: "sepolia", feedUrl: "https://feed.test",
    rpcUrl: "https://header.test", proofRpcUrl: "https://proof.test" }, profile);
  source.stream(true); source.head(123);
  const pending = source.acquire(123);
  await Promise.resolve();
  assert.deepEqual(gets, []);
  source.receive(envelope(123));
  assert.equal((await pending).checkpoint.block_number, 123);
  assert.deepEqual(gets, []);
  source.head(124);
  const missed = source.acquire(124);
  source.stream(false);
  assert.equal((await missed).checkpoint.block_number, 124);
  assert.deepEqual(gets, ["proofs/124"]);
  assert.throws(() => source.receive({ ...envelope(125), block_hash: "0x999" }), /malformed feed proof/);
});

test("checkpoint requests run concurrently and retain the trusted header boundary", async (t) => {
  const profile = resolveProfile("sepolia");
  const net = new PublicTransport("https://feed.test", () => {});
  let release!: () => void;
  const gate = new Promise<void>((resolve) => { release = resolve; });
  const methods: string[] = [];
  let chain = `0x${Buffer.from(profile.chainId).toString("hex")}`;
  t.mock.method(net, "rpc", async (_url: string, method: string, params: unknown[]) => {
    methods.push(method);
    if (method === "starknet_chainId") return chain;
    assert.deepEqual(params[0], { block_number: 123 });
    if (method === "starknet_getStorageProof") return { proof: "untrusted" };
    await gate;
    return { block_number: 123, block_hash: "0x123", new_root: "0x456", status: "ACCEPTED_ON_L2" };
  });
  const opts = { network: "sepolia" as const, feedUrl: "https://feed.test",
    rpcUrl: "https://header.test", proofRpcUrl: "https://proof.test", proofSource: "rpc" as const };
  const pending = new CheckpointSource(net, opts, profile).acquire(123);
  // Holding the header response must not hold up the proof or chain request.
  try {
    assert.deepEqual(methods.sort(), ["starknet_chainId", "starknet_getBlockWithTxHashes", "starknet_getStorageProof"]);
  } finally { release(); }
  const result = await pending;
  assert.equal(result.checkpoint.block_hash, "0x123");
  assert.equal(result.checkpoint.state_root, "0x456");
  chain = "0x1";
  await assert.rejects(new CheckpointSource(net, opts, profile).acquire(123), /CHAIN_MISMATCH/);
});

test("a prepared checkpoint is shared and a superseded request is cancelled", async (t) => {
  const profile = resolveProfile("sepolia");
  const net = new PublicTransport("https://feed.test", () => {});
  const headers: number[] = [];
  t.mock.method(net, "rpc", async (_url: string, method: string, params: any[], signal?: AbortSignal) => {
    if (method === "starknet_chainId") return `0x${Buffer.from(profile.chainId).toString("hex")}`;
    if (method === "starknet_getStorageProof") return {};
    const block = params[0].block_number;
    headers.push(block);
    if (block === 123) await new Promise((_resolve, reject) => {
      signal!.addEventListener("abort", () => reject(new Error("cancelled")), { once: true });
    });
    return { block_number: block, block_hash: "0x123", new_root: "0x456", status: "ACCEPTED_ON_L2" };
  });
  const source = new CheckpointSource(net, { network: "sepolia", feedUrl: "https://feed.test",
    rpcUrl: "https://header.test", proofRpcUrl: "https://proof.test", proofSource: "rpc" as const }, profile);
  source.prepare(123);
  const old = source.acquire(123);
  source.prepare(124);
  await assert.rejects(old, /cancelled/);
  assert.equal((await source.acquire(124)).checkpoint.block_number, 124);
  assert.deepEqual(headers, [123, 124], "no duplicate acquisition or unbounded pending work");
});

test("new-head refusal retries are immediate, bounded and stay on the trusted endpoint", async (t) => {
  const profile = resolveProfile("sepolia");
  const net = new PublicTransport("https://feed.test", () => {});
  let headers = 0, failures = 2;
  t.mock.method(net, "rpc", async (url: string, method: string) => {
    if (method === "starknet_chainId") return `0x${Buffer.from(profile.chainId).toString("hex")}`;
    if (method === "starknet_getStorageProof") return {};
    assert.equal(url, "https://header.test");
    if (++headers <= failures) throw new Error("RPC_UNAVAILABLE: 24 Block not found");
    return { block_number: 123, block_hash: "0x123", new_root: "0x456", status: "ACCEPTED_ON_L2" };
  });
  const opts = { network: "sepolia" as const, feedUrl: "https://feed.test",
    rpcUrl: "https://header.test", proofRpcUrl: "https://proof.test", proofSource: "rpc" as const };
  assert.equal((await new CheckpointSource(net, opts, profile).acquire(123)).checkpoint.block_number, 123);
  assert.equal(headers, 3);
  headers = 0; failures = 10;
  await assert.rejects(new CheckpointSource(net, opts, profile).acquire(123), /RPC_UNAVAILABLE: 24/);
  assert.equal(headers, 3);
});
