import { test } from "node:test";
import assert from "node:assert/strict";
import {
  Signer,
  constants,
  type Signature,
  type InvocationsSignerDetails,
} from "starknet";
import { SubmissionSigner } from "../src/submission-signer.ts";

test("the SDK hash is durably recorded before signing; failed storage prevents signing", async (t) => {
  // The cryptographic signing operation is replaced. No real transaction is signed.
  const base = Signer.prototype as unknown as {
    signRaw(hash: string): Promise<Signature>;
  };
  const order: string[] = [];
  t.mock.method(base, "signRaw", async (hash: string): Promise<Signature> => {
    order.push(`sign:${hash}`);
    return [];
  });
  const details: InvocationsSignerDetails = {
    walletAddress: "0x123",
    cairoVersion: "1",
    chainId: constants.StarknetChainId.SN_SEPOLIA,
    version: "0x3",
    nonce: 0,
    tip: 0,
    paymasterData: [],
    accountDeploymentData: [],
    nonceDataAvailabilityMode: "L1",
    feeDataAvailabilityMode: "L1",
    resourceBounds: {
      l1_gas: { max_amount: 1n, max_price_per_unit: 1n },
      l2_gas: { max_amount: 1n, max_price_per_unit: 1n },
      l1_data_gas: { max_amount: 1n, max_price_per_unit: 1n },
    },
  };
  const calls = [
    {
      contractAddress: "0x456",
      entrypoint: "transfer",
      calldata: ["0x789", "1", "0"],
    },
  ];
  const signer = new SubmissionSigner("0x1", async (hash) => {
    await Promise.resolve();
    order.push(`saved:${hash}`);
  });
  await signer.signTransaction(calls, details);
  assert.equal(order.length, 2);
  assert.equal(order[0]!.replace("saved:", ""), order[1]!.replace("sign:", ""));
  assert(order[0]!.startsWith("saved:0x"));
  order.length = 0;
  const failed = new SubmissionSigner("0x1", async () => {
    throw new Error("Storage unavailable");
  });
  await assert.rejects(
    () => failed.signTransaction(calls, details),
    /Storage unavailable/,
  );
  assert.deepEqual(order, []);
});
