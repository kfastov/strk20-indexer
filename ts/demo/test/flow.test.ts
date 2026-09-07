import { test } from "node:test";
import assert from "node:assert/strict";
import { walletStep } from "../src/flow.ts";

test("a completed wallet can repeat every operation without deleting history", () => {
  const wallet = { completed: { shield: { hash: "0x1", block: 10 },
    transfer: { hash: "0x2", block: 20 }, withdraw: { hash: "0x3", block: 30 } } };
  const original = structuredClone(wallet);
  assert.equal(walletStep(wallet, undefined).action, "discover");
  assert.equal(walletStep(wallet, "0x3").action, "shield");
  for (const action of ["shield", "transfer", "withdraw", "discover"] as const)
    assert.equal(walletStep(wallet, undefined, action).action, action);
  assert.deepEqual(wallet, original);
  wallet.completed.shield = { hash: "0x4", block: 40 };
  assert.equal(walletStep(wallet, "0x3").action, "discover");
  assert.equal(walletStep(wallet, "0x4").action, "transfer", "a new deposit starts the next cycle");
  wallet.completed.transfer = { hash: "0x5", block: 50 };
  assert.equal(walletStep(wallet, "0x5").action, "withdraw");
});

test("pending transactions take precedence over every selected operation", () => {
  const wallet = { completed: {}, pending: { action: "transfer" as const, hash: "0x123" } };
  for (const action of ["shield", "transfer", "withdraw", "discover"] as const)
    assert.equal(walletStep(wallet, undefined, action).action, "resume");
  assert.equal(walletStep({ completed: {} }, undefined).action, "shield");
});
