// Independent on-chain verification of the shield run, plus note discovery with
// the derived viewing key. Read-only except for appending observed facts to the
// operator's key file.
//
//   node verify.mjs [tx_hash]      # defaults to transactions.apply_actions.hash
import { RpcProvider, Account, constants, ec } from "starknet";
import { createPrivateTransfers, ProvingServiceProofProvider, IndexerDiscoveryProvider } from "@starkware-libs/starknet-privacy-sdk";
import {
  RPC, POOL, STRK, PROVER_URL, DISCOVERY_URL, SEPOLIA_CHAIN_ID,
  loadAccount, deriveViewingKey, readKeyFile, writeKeyFile, keyFilePath, rpc, fmt,
} from "./lib.mjs";

const saved = readKeyFile();
const TX = process.argv[2] ?? saved.transactions?.apply_actions?.hash;
if (!TX) throw new Error("no transaction hash: pass one on the command line or run shield.mjs first");

const provider = new RpcProvider({ nodeUrl: RPC, batch: false });
const a = loadAccount();
const viewingKey = BigInt(saved.viewing_key);
console.log("viewing key in the key file matches a fresh derivation:", viewingKey === deriveViewingKey(a.privateKey, SEPOLIA_CHAIN_ID, POOL));

// ── 1. receipt + the three target events ───────────────────────────────────
// Selectors are starknet_keccak of the event names; hard-coded so a decoder bug
// cannot make a missing event look present.
const TARGETS = {
  EncNoteCreated: "0x023c20207be8b1ef4430c25eef8ce779c9745ebe04139555ae81bd4f8fdd6ec5",
  ViewingKeySet: "0x01321a492485b4f19851fb787ab3800a0030b595332cba93cd5fe40dfb5a4daf",
  Deposit: "0x009149d2123147c5f43d258257fef0b7b969db78269369ebcf5ebb9eef8592f2",
};
const rc = await rpc("starknet_getTransactionReceipt", [TX]);
console.log(`\ntx ${TX}\n  block ${rc.block_number}  ${rc.execution_status} ${rc.finality_status}  fee ${fmt(BigInt(rc.actual_fee.amount))} STRK`);
const blk = await rpc("starknet_getBlockWithTxHashes", [{ block_number: rc.block_number }]);
console.log(`  block hash ${blk.block_hash}  ts ${new Date(blk.timestamp * 1000).toISOString()}  tx_index ${blk.transactions.indexOf(TX)}`);
for (const [name, selector] of Object.entries(TARGETS)) {
  const hits = (rc.events ?? []).filter((e) => BigInt(e.from_address) === BigInt(POOL) && BigInt(e.keys[0]) === BigInt(selector));
  console.log(`  ${name.padEnd(15)} ${selector}  -> ${hits.length} event(s)` + (hits.length ? `  keys=${hits[0].keys.length} data=${hits[0].data.length}` : "  *** MISSING ***"));
  for (const h of hits) console.log(`      keys: ${JSON.stringify(h.keys)}\n      data: ${JSON.stringify(h.data)}`);
}

// ── 2. registration is visible in pool state ───────────────────────────────
const [pk] = await provider.callContract({ contractAddress: POOL, entrypoint: "get_public_key", calldata: [a.address] }, "latest");
const expected = ec.starkCurve.getStarkKey("0x" + viewingKey.toString(16));
console.log(`\npool get_public_key(${a.address})\n  on-chain ${pk}\n  expected ${expected}\n  match: ${BigInt(pk) === BigInt(expected)}`);

// ── 3. the note is discoverable with the derived viewing key ───────────────
const transfers = createPrivateTransfers({
  account: new Account({ provider, address: a.address, signer: a.privateKey, cairoVersion: "1" }),
  viewingKeyProvider: { getViewingKey: async () => viewingKey },
  provingProvider: new ProvingServiceProofProvider(PROVER_URL, constants.StarknetChainId.SN_SEPOLIA, { requestTimeoutMs: 60_000 }),
  discoveryProvider: new IndexerDiscoveryProvider(DISCOVERY_URL, POOL),
  poolContractAddress: POOL,
});
const found = await transfers.discoverNotes();
console.log("\ndiscoverNotes at", JSON.stringify(found.timestamp));
for (const [token, notes] of found.notes.entries()) {
  console.log(`  token 0x${token.toString(16)}${BigInt(token) === BigInt(STRK) ? " (STRK)" : ""}: ${notes.length} note(s)`);
  for (const n of notes) console.log(`    id ${n.id}  amount ${fmt(n.amount)} STRK  created ${n.created}  sender ${n.sender}  open ${n.open ?? false}`);
}

// ── 4. record the verified facts ───────────────────────────────────────────
saved.transactions.apply_actions = {
  ...saved.transactions.apply_actions,
  hash: TX,
  block: rc.block_number,
  block_hash: blk.block_hash,
  block_timestamp: blk.timestamp,
  tx_index: blk.transactions.indexOf(TX),
  events_observed: Object.entries(TARGETS)
    .filter(([, s]) => (rc.events ?? []).some((e) => BigInt(e.from_address) === BigInt(POOL) && BigInt(e.keys[0]) === BigInt(s)))
    .map(([n]) => n),
};
saved.pool_public_key_onchain = pk;
writeKeyFile(saved);
console.log("\nupdated", keyFilePath());
