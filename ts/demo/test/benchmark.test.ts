import { test } from "node:test";
import assert from "node:assert/strict";
import {
  AddressMap,
  IndexerDiscoveryProvider,
  Witness,
  type DiscoveryProviderInterface,
} from "@starkware-libs/starknet-privacy-sdk";
import { observeAt, type Comparison } from "../src/benchmark.ts";
import { newWallet } from "../src/wallet.ts";

test("comparison normalizes SDK note IDs but still detects different witnesses", async (t) => {
  type Notes = Awaited<ReturnType<DiscoveryProviderInterface["discoverNotes"]>>;
  const found = (id: bigint | string, salt: bigint): Notes => ({
    timestamp: 99,
    notes: new AddressMap([
      [
        1n,
        [{
          id,
          amount: 10n,
          created: 80,
          sender: 2n,
          open: false,
          witness: new Witness(3n, 0, salt),
        }],
      ],
    ]),
    cursor: { blockId: 99, incomingChannels: new AddressMap() },
  });
  let salt = 4n;
  t.mock.method(IndexerDiscoveryProvider.prototype, "discoverNotes", async () =>
    found("0x0a", salt),
  );
  const local = {
    discoverNotes: async () => found(10n, 4n),
  };
  const wallet = newWallet("sepolia");
  let comparison: Comparison | undefined;
  await observeAt(wallet, local, 99, true, (result) => {
    comparison = result;
  });
  assert.equal(comparison?.equal, true);
  salt = 5n;
  await observeAt(wallet, local, 99, true, (result) => {
    comparison = result;
  });
  assert.equal(comparison?.equal, false);
});
