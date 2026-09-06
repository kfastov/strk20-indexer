import { test } from "node:test";
import assert from "node:assert/strict";
import { CheckpointSource } from "../src/checkpoint.ts";
import { PublicTransport } from "../src/net.ts";
import { resolveProfile } from "../src/profiles.ts";

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
    rpcUrl: "https://header.test", proofRpcUrl: "https://proof.test" };
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
