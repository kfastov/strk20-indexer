import { test } from "node:test";
import assert from "node:assert/strict";
import "fake-indexeddb/auto";
import { StateCache } from "../../strk20-discovery/src/storage.ts";
import {
  newWallet,
  loadWallet,
  saveWallet,
  validateWallet,
} from "../src/wallet.ts";

test("wallet backup preserves both keys and survives clearing discovery state", async () => {
  const wallet = newWallet("sepolia");
  wallet.completed.shield = { hash: "0x123", block: 100 };
  wallet.pending = { action: "transfer", hash: "0x456" };
  const backup = JSON.parse(JSON.stringify(wallet));
  assert.deepEqual(validateWallet(backup), wallet);
  await saveWallet(wallet);
  await new StateCache("strk20-folded-v2:wallet-isolation-test").clear();
  assert.deepEqual(await loadWallet("sepolia"), wallet);
  const mainnet = newWallet("mainnet");
  await saveWallet(mainnet);
  assert.deepEqual(await loadWallet("sepolia"), wallet);
  assert.deepEqual(await loadWallet("mainnet"), mainnet);
  for (const patch of [
    { network: "mainnet" },
    { viewingKey: "0x1" },
    { address: "0x1" },
    { completed: { shield: { hash: "invalid", block: 100 } } },
    { pending: { action: "unknown", hash: "0x456" } },
  ])
    assert.throws(() => validateWallet({ ...backup, ...patch }));
});
