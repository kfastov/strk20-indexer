// Verify the spend: the nullifier in the NoteUsed event, the new note, the pool
// class hash in effect at that block, and discovery after the spend.
//
//   node verify-spend.mjs [tx_hash]   # defaults to transactions.spend_transfer.hash
//
// The interesting assertion is the nullifier: the client predicts it from the
// spent note before the transaction exists, and the contract emits it. If the
// two agree, the nullifier formula is confirmed by the chain itself.
import { RpcProvider, Account, constants } from "starknet";
import { createPrivateTransfers, ProvingServiceProofProvider, IndexerDiscoveryProvider } from "@starkware-libs/starknet-privacy-sdk";
import {
  RPC, POOL, STRK, PROVER_URL, DISCOVERY_URL, SEPOLIA_CHAIN_ID,
  loadAccount, deriveViewingKey, readKeyFile, writeKeyFile, keyFilePath, rpc, fmt,
} from "./lib.mjs";

const saved = readKeyFile();
const SHAPE = process.env.SHAPE ?? "transfer";
const TX = process.argv[2] ?? saved.transactions?.[`spend_${SHAPE}`]?.hash;
if (!TX) throw new Error(`no spend transaction hash: pass one on the command line or run spend.mjs first`);
// The note that was spent, and the block its creation landed in — both recorded
// by the earlier runs, so nothing here is hard-coded to one particular demo.
const OLD_NOTE = saved.transactions?.[`spend_${SHAPE}`]?.spent_note_id;
const SHIELD_BLOCK = saved.transactions?.apply_actions?.block;
const EXPECTED_NULLIFIER = process.env.EXPECTED_NULLIFIER ?? saved.transactions?.[`spend_${SHAPE}`]?.nullifier ?? null;

const provider = new RpcProvider({ nodeUrl: RPC, batch: false });
const a = loadAccount();
const viewingKey = BigInt(saved.viewing_key);

const rc = await rpc("starknet_getTransactionReceipt", [TX]);
const blk = await rpc("starknet_getBlockWithTxHashes", [{ block_number: rc.block_number }]);
console.log(`tx ${TX}`);
console.log(`  block ${rc.block_number} tx_index ${blk.transactions.indexOf(TX)}  ${rc.execution_status} ${rc.finality_status}`);
console.log(`  block hash ${blk.block_hash}  ts ${new Date(blk.timestamp * 1000).toISOString()}`);
console.log(`  fee ${fmt(BigInt(rc.actual_fee.amount))} STRK`);

// class hash in effect AT that block, vs at the shield block
const clsAt = await rpc("starknet_getClassHashAt", [{ block_number: rc.block_number }, POOL]);
console.log(`\npool class at block ${rc.block_number}: ${clsAt}`);
if (SHIELD_BLOCK) {
  const clsPrev = await rpc("starknet_getClassHashAt", [{ block_number: SHIELD_BLOCK }, POOL]);
  console.log(`pool class at block ${SHIELD_BLOCK} (the shield block): ${clsPrev}`);
  console.log(`  -> upgraded between the two: ${BigInt(clsAt) !== BigInt(clsPrev)}`);
}

const NULL_SEL = "0x0247fc60d782e0094e7f98c47f277d92a3345d07a436f1f56b27a9b62be2322e"; // NoteUsed
const ENC_SEL = "0x023c20207be8b1ef4430c25eef8ce779c9745ebe04139555ae81bd4f8fdd6ec5"; // EncNoteCreated
const poolEvents = (sel) => (rc.events ?? []).filter((e) => BigInt(e.from_address) === BigInt(POOL) && BigInt(e.keys[0]) === BigInt(sel));
const used = poolEvents(NULL_SEL);
const made = poolEvents(ENC_SEL);
console.log(`\nNoteUsed events: ${used.length}`);
for (const e of used) {
  console.log(`  nullifier (event keys[1]): ${e.keys[1]}`);
  if (EXPECTED_NULLIFIER) console.log(`  matches client prediction ${EXPECTED_NULLIFIER}: ${BigInt(e.keys[1]) === BigInt(EXPECTED_NULLIFIER)}`);
}
console.log(`EncNoteCreated events: ${made.length}`);
for (const e of made) console.log(`  new note id: ${e.keys[1]}\n  enc data:    ${JSON.stringify(e.data)}`);

// contract-level state checks
const call = (e, d = []) => provider.callContract({ contractAddress: POOL, entrypoint: e, calldata: d }, "latest");
const nullifier = used[0]?.keys[1] ?? EXPECTED_NULLIFIER;
if (nullifier) {
  const [nExists] = await call("nullifier_exists", [nullifier]);
  console.log(`\nnullifier_exists(${nullifier.slice(0, 14)}…) = ${BigInt(nExists) === 1n}`);
}
for (const [label, id] of [["old note", OLD_NOTE], ["new note", made[0]?.keys[1]]]) {
  if (!id) continue;
  const r = await call("get_note", [id]);
  console.log(`get_note(${label} ${String(id).slice(0, 14)}…) packed_value=${r[0]} token=${r[1]}`);
}
console.log("(a spent note's slot is NOT cleared — spentness lives only in nullifiers / NoteUsed)");

// discovery after the spend
const transfers = createPrivateTransfers({
  account: new Account({ provider, address: a.address, signer: a.privateKey, cairoVersion: "1" }),
  viewingKeyProvider: { getViewingKey: async () => viewingKey },
  provingProvider: new ProvingServiceProofProvider(PROVER_URL, constants.StarknetChainId.SN_SEPOLIA, { requestTimeoutMs: 60_000 }),
  discoveryProvider: new IndexerDiscoveryProvider(DISCOVERY_URL, POOL),
  poolContractAddress: POOL,
});
const found = await transfers.discoverNotes();
console.log("\ndiscoverNotes AFTER the spend:");
for (const [token, notes] of found.notes.entries()) {
  console.log(`  token 0x${token.toString(16)}${BigInt(token) === BigInt(STRK) ? " (STRK)" : ""}: ${notes.length} unspent note(s)`);
  for (const n of notes) console.log(`    id ${n.id}  ${fmt(n.amount)} STRK  created ${n.created}  open ${n.open ?? false}`);
}
console.log("viewing key file derivation still matches:", viewingKey === deriveViewingKey(a.privateKey, SEPOLIA_CHAIN_ID, POOL));

saved.transactions[`spend_${SHAPE}`] = {
  ...saved.transactions[`spend_${SHAPE}`],
  hash: TX,
  block: rc.block_number,
  block_hash: blk.block_hash,
  block_timestamp: blk.timestamp,
  tx_index: blk.transactions.indexOf(TX),
  nullifier: used[0]?.keys[1],
  new_note_id: made[0]?.keys[1],
  new_note_enc_data: made[0]?.data,
  pool_class_hash_at_block: clsAt,
};
writeKeyFile(saved);
console.log("\nupdated", keyFilePath());
