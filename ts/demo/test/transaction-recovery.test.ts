import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import "fake-indexeddb/auto";
import {
  Account,
  RpcProvider,
  Signer,
  constants,
  type Signature,
} from "starknet";
import { NodeDiscoveryProvider } from "strk20-discovery/node";
import { Transactions } from "../src/transactions.ts";
import { Operations } from "../src/operations.ts";
import { newWallet, loadWallet } from "../src/wallet.ts";

test("lost deployment response retains the precomputed hash and resumes without another send", async (t) => {
  const directory = await mkdtemp(join(tmpdir(), "strk20-demo-recovery-"));
  const discovery = new NodeDiscoveryProvider({
    network: "sepolia",
    feedUrl: "https://unused.invalid",
    cacheDirectory: directory,
  });
  let sends = 0,
    signatures = 0,
    signedHash = "";
  try {
    await discovery.ready;
    t.mock.method(globalThis, "fetch", async () => {
      throw new Error("Unexpected network request");
    });
    // Keep actual Account/Signer transaction construction; disable cryptographic
    // signing and replace every RPC. This test cannot submit a transaction.
    t.mock.method(
      Signer.prototype as unknown as {
        signRaw(hash: string): Promise<Signature>;
      },
      "signRaw",
      async (hash: string): Promise<Signature> => {
        signedHash = hash;
        signatures++;
        return [];
      },
    );
    t.mock.method(
      RpcProvider.prototype,
      "getChainId",
      async () => constants.StarknetChainId.SN_SEPOLIA,
    );
    t.mock.method(RpcProvider.prototype, "getClass", async () => ({}));
    t.mock.method(RpcProvider.prototype, "callContract", async () => [
      String(10n ** 20n),
      "0",
    ]);
    t.mock.method(Account.prototype, "estimateAccountDeployFee", async () => ({
      resourceBounds: {
        l1_gas: { max_amount: 1n, max_price_per_unit: 1n },
        l2_gas: { max_amount: 1n, max_price_per_unit: 1n },
        l1_data_gas: { max_amount: 1n, max_price_per_unit: 1n },
      },
    }));
    t.mock.method(RpcProvider.prototype, "deployAccountContract", async () => {
      sends++;
      assert.equal((await loadWallet("sepolia"))?.pending?.hash, signedHash);
      throw new Error("Response lost after broadcast");
    });
    const wallet = newWallet("sepolia");
    const operations = new Operations(() => {});
    await assert.rejects(
      () => new Transactions(wallet, discovery, operations).deploy(),
      /Response lost/,
    );
    const restored = await loadWallet("sepolia");
    assert(restored?.pending?.hash);
    assert.equal(restored.pending.hash, signedHash);
    t.mock.method(
      RpcProvider.prototype,
      "waitForTransaction",
      async (hash: string) => {
        assert.equal(hash, signedHash);
        return { isSuccess: () => true, block_number: 100 };
      },
    );
    await new Transactions(restored, discovery, operations).resume();
    assert.equal(
      (await loadWallet("sepolia"))?.completed.deploy?.hash,
      signedHash,
    );
    assert.equal((await loadWallet("sepolia"))?.pending, undefined);
    assert.equal(sends, 1);
    assert.equal(signatures, 1);
  } finally {
    await discovery.close();
    await rm(directory, { recursive: true, force: true });
  }
});
