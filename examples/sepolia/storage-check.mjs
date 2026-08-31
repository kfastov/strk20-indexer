// Does a pool upgrade move the storage layout? Recompute the notes/nullifiers
// slots from the Cairo storage declaration and read them either side of the
// block that wrote them.
//
//   notes:      Map<felt252, Note>    (packages/privacy/src/privacy.cairo)
//   nullifiers: Map<felt252, bool>
//   slot = pedersen(sn_keccak(var_name), key) mod (2^251 - 256)
//
// Discovery walks storage, not events, so an ABI diff cannot answer this — only
// a note written by the new class can. Read-only; no keys involved.
//
//   node storage-check.mjs                       # uses the recorded run
//   NOTE_ID=0x… NOTE_BLOCK=… node storage-check.mjs
import { hash, num } from "starknet";
import { POOL, rpc, readKeyFile } from "./lib.mjs";

const ADDR_BOUND = 2n ** 251n - 256n;
const slot = (varName, key) => num.toHex(BigInt(hash.computePedersenHash(num.toHex(hash.starknetKeccak(varName)), key)) % ADDR_BOUND);
const at = (addr, block) => rpc("starknet_getStorageAt", [POOL, addr, block === "latest" ? "latest" : { block_number: block }]);

// Targets come from the recorded runs unless overridden on the command line.
let targets = [];
let nullifier = null;
let nullifierBlock = null;
if (process.env.NOTE_ID && process.env.NOTE_BLOCK) {
  targets = [["note (explicit)", process.env.NOTE_ID, Number(process.env.NOTE_BLOCK)]];
  nullifier = process.env.NULLIFIER ?? null;
  nullifierBlock = process.env.NULLIFIER_BLOCK ? Number(process.env.NULLIFIER_BLOCK) : null;
} else {
  const t = readKeyFile().transactions ?? {};
  const spend = t.spend_transfer ?? t.spend_withdraw;
  if (t.apply_actions?.block && spend?.spent_note_id) targets.push(["note written by the pre-upgrade class", spend.spent_note_id, t.apply_actions.block]);
  if (spend?.new_note_id && spend?.block) targets.push(["note written by the post-upgrade class", spend.new_note_id, spend.block]);
  nullifier = spend?.nullifier ?? null;
  nullifierBlock = spend?.block ?? null;
}
if (targets.length === 0) throw new Error("nothing to check: run shield.mjs + spend.mjs + the verify scripts first, or pass NOTE_ID/NOTE_BLOCK");

console.log("slot formula: pedersen(sn_keccak(var_name), key) mod (2^251-256)\n");

for (const [label, noteId, createdAt] of targets) {
  const s = slot("notes", noteId);
  const before = await at(s, createdAt - 1);
  const after = await at(s, createdAt);
  console.log(label);
  console.log(`  note id   ${noteId}`);
  console.log(`  slot      ${s}`);
  console.log(`  @${createdAt - 1} (before) ${before}`);
  console.log(`  @${createdAt} (after)  ${after}`);
  console.log(`  @latest            ${await at(s, "latest")}`);
  console.log(`  -> slot populated exactly at the creation block: ${BigInt(before) === 0n && BigInt(after) !== 0n}`);
  // Note is {packed_value, token} — two consecutive slots. token reads 0x0:
  // the token lives inside packed_value, not in a separate slot.
  console.log(`  slot+1 (token field) @latest: ${await at(num.toHex(BigInt(s) + 1n), "latest")}\n`);
}

if (nullifier && nullifierBlock) {
  const ns = slot("nullifiers", nullifier);
  console.log("nullifier", nullifier);
  console.log(`  slot ${ns}`);
  console.log(`  @${nullifierBlock - 1} ${await at(ns, nullifierBlock - 1)}`);
  console.log(`  @${nullifierBlock} ${await at(ns, nullifierBlock)}`);
  console.log(`  @latest   ${await at(ns, "latest")}`);
}
