import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import "fake-indexeddb/auto";
import { once } from "node:events";
import { WebSocketServer } from "ws";
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
    const server = new WebSocketServer({ port: 0, host: "127.0.0.1" });
    await once(server, "listening");
    t.after(async () => {
      for (const socket of server.clients) socket.terminate();
      await new Promise<void>((resolve) => server.close(() => resolve()));
    });
    const address = server.address();
    assert(address && typeof address !== "string");
    const url = `ws://127.0.0.1:${address.port}`;
    const NativeSocket = globalThis.WebSocket;
    t.mock.property(globalThis, "WebSocket", class extends NativeSocket {
      constructor() { super(url); }
    });
    let receiptAvailable = false;
    t.mock.method(RpcProvider.prototype, "getTransactionReceipt", async (hash: string) => {
      assert.equal(hash, signedHash);
      if (!receiptAvailable) throw new Error("Receipt connection failed");
      return { transaction_hash: signedHash, execution_status: "SUCCEEDED",
        finality_status: "ACCEPTED_ON_L2", block_number: 100 };
    });
    server.on("connection", (socket) => socket.on("message", (bytes) => {
      const request = JSON.parse(String(bytes));
      assert.equal(BigInt(request.params.transaction_hash), BigInt(signedHash));
      const status = { execution_status: "SUCCEEDED", finality_status: "ACCEPTED_ON_L2" };
      if (request.method === "starknet_subscribeTransactionStatus") {
        socket.send(JSON.stringify({ jsonrpc: "2.0", id: request.id, result: "sub" }));
        socket.send(JSON.stringify({ jsonrpc: "2.0", method: "starknet_subscriptionTransactionStatus",
          params: { subscription_id: "sub", result: { transaction_hash: signedHash, status } } }));
      } else assert.fail(`Unexpected WebSocket method: ${request.method}`);
    }));
    await assert.rejects(new Transactions(restored, discovery, operations).resume(), /Receipt connection failed/);
    assert.equal((await loadWallet("sepolia"))?.pending?.hash, signedHash);
    receiptAvailable = true;
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
