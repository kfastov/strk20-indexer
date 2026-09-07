import { test } from "node:test";
import assert from "node:assert/strict";
import { parseRpcSettings } from "../src/rpc-settings.ts";

test("a private QuickNode setting derives matching HTTP, WS and proof endpoints", () => {
  for (const network of ["mainnet", "sepolia"] as const) {
    const base = `https://fixture.strk-${network}.quiknode.pro/test-key`;
    for (const value of [base, base + "/", base + "/rpc/v0_9"]) {
      assert.deepEqual(parseRpcSettings(network, value), {
        rpc: base + "/rpc/v0_9", ws: base.replace("https:", "wss:") + "/rpc/v0_9", proofRpc: base + "/rpc/v0_9",
      });
    }
  }
  assert.equal(parseRpcSettings("mainnet", " "), undefined);
});

test("a private endpoint cannot silently select the wrong network or another destination", () => {
  for (const value of [
    "https://fixture.strk-mainnet.quiknode.pro/test-key",
    "https://fixture.strk-sepolia.quiknode.pro.evil.example/test-key",
    "http://fixture.strk-sepolia.quiknode.pro/test-key",
    "https://user:password@fixture.strk-sepolia.quiknode.pro/test-key",
    "https://fixture.strk-sepolia.quiknode.pro/test-key?redirect=1",
  ]) assert.throws(() => parseRpcSettings("sepolia", value));
});
