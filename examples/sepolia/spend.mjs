// Run 2 — spend the shielded note: private self-transfer (nullifies the old
// note, creates a fresh EncNoteCreated). TESTNET ONLY.
//
// Differences from shield.mjs:
//  - no deposit, so the ERC20 allowance only has to cover the pool fee
//    (collect_fee charges the caller on EVERY apply_actions, not just deposits)
//  - approve is batched into the SAME transaction as apply_actions, so the run
//    costs one transaction fee instead of two. This works because the proof
//    facts bind the pool's own action span, not the transaction's call list.
//  - SHAPE=transfer (default) | withdraw
//
// Env: see .env.example. DRY_RUN=1 stops before the submit.
import fs from "node:fs";
import { RpcProvider, Account, constants, cairo, TransactionFinalityStatus, hash } from "starknet";
import { createPrivateTransfers, ProvingServiceProofProvider, IndexerDiscoveryProvider } from "@starkware-libs/starknet-privacy-sdk";
import {
  RPC, POOL, STRK, PROVER_URL, DISCOVERY_URL, SEPOLIA_CHAIN_ID, DRY_RUN, PROVING_BLOCK_DEPTH,
  loadAccount, deriveViewingKey, recordTransaction, outFile, fmt, log, boundsCeiling,
} from "./lib.mjs";

const SHAPE = process.env.SHAPE ?? "transfer";

const WAIT_OPTIONS = {
  successStates: [
    TransactionFinalityStatus.PRE_CONFIRMED,
    TransactionFinalityStatus.ACCEPTED_ON_L2,
    TransactionFinalityStatus.ACCEPTED_ON_L1,
  ],
  retryInterval: 2000,
};

const provider = new RpcProvider({ nodeUrl: RPC, batch: false });
const a = loadAccount();
const account = new Account({ provider, address: a.address, signer: a.privateKey, cairoVersion: "1" });
const viewingKey = deriveViewingKey(a.privateKey, SEPOLIA_CHAIN_ID, POOL);

const view = (c, e, d = []) => provider.callContract({ contractAddress: c, entrypoint: e, calldata: d }, "latest");
const u256 = async (c, e, d) => { const [lo, hi] = await view(c, e, d); return BigInt(lo) + (BigInt(hi) << 128n); };
const balance = () => u256(STRK, "balance_of", [a.address]);
const allowance = () => u256(STRK, "allowance", [a.address, POOL]);

const chainId = await provider.getChainId();
if (chainId !== constants.StarknetChainId.SN_SEPOLIA) throw new Error(`REFUSING: chain id ${chainId} is not SN_SEPOLIA`);

const classHash = await provider.getClassHashAt(POOL, "latest");
const [verRaw] = await view(POOL, "get_version");
const poolFee = BigInt((await view(POOL, "get_fee_amount"))[0]);
const bal0 = await balance();
log(`chain SN_SEPOLIA head ${await provider.getBlockNumber()}`);
log(`pool class ${classHash}  version ${BigInt(verRaw)} ("${Buffer.from(BigInt(verRaw).toString(16), "hex").toString()}")`);
log(`account ${a.address} balance ${fmt(bal0)} STRK; pool fee ${fmt(poolFee)} STRK; allowance ${fmt(await allowance())} STRK`);

// ── SDK ────────────────────────────────────────────────────────────────────
const transfers = createPrivateTransfers({
  account,
  viewingKeyProvider: { getViewingKey: async () => viewingKey },
  provingProvider: new ProvingServiceProofProvider(PROVER_URL, constants.StarknetChainId.SN_SEPOLIA, { requestTimeoutMs: 180_000 }),
  discoveryProvider: new IndexerDiscoveryProvider(DISCOVERY_URL, POOL),
  poolContractAddress: POOL,
});

const found = await transfers.discoverNotes();
const notes = found.notes.get(BigInt(STRK)) ?? [];
log(`discovered ${notes.length} STRK note(s) BEFORE the spend:`);
for (const n of notes) log(`   id ${n.id} amount ${fmt(n.amount)} STRK created ${n.created} open ${n.open ?? false}`);
if (notes.length === 0) throw new Error("no spendable note found — run shield.mjs first, or check the viewing key");
const spend = notes[0];

// ── build ──────────────────────────────────────────────────────────────────
const chain = transfers
  .build({ autoRegister: true, autoSetup: true, autoDiscover: { notes: "refresh", channels: "refresh" }, autoSelectNotes: "naive" })
  .surplusTo(a.address)
  .with(STRK, (t) => {
    if (SHAPE === "withdraw") t.withdraw({ amount: spend.amount / 2n, recipient: a.address });
    else t.transfer({ recipient: a.address, amount: spend.amount });
  });

const head = await provider.getBlockNumber();
const provingBlockId = head - PROVING_BLOCK_DEPTH;
log(`shape=${SHAPE}; head ${head}; proving at block ${provingBlockId} (head-${PROVING_BLOCK_DEPTH})`);
const invocation = await chain.createProofInvocation({ provingBlockId });
log("invocation built; warnings:", JSON.stringify(invocation.warnings));

const t0 = Date.now();
const { callAndProof, warnings } = await transfers.executeWithInvocation(invocation, provingBlockId);
log(`proved in ${((Date.now() - t0) / 1000).toFixed(1)}s; proof ${Math.round((callAndProof.proof.data?.length ?? 0) * 0.75)} bytes, ${callAndProof.proof.proofFacts.length} facts`);
log("screening attestation present:", Boolean(callAndProof.proof.additionalData?.signature), "| warnings:", JSON.stringify(warnings));
log("apply_actions calldata felts:", callAndProof.call.calldata.length);
fs.writeFileSync(
  outFile("last-spend-callandproof.json"),
  JSON.stringify({ call: callAndProof.call, proofFacts: callAndProof.proof.proofFacts, output: callAndProof.proof.output, additionalData: callAndProof.proof.additionalData, provingBlockId, classHash }, null, 1),
);

// ── approve (batched) + estimate ───────────────────────────────────────────
const calls = [];
if ((await allowance()) < poolFee) {
  calls.push({ contractAddress: STRK, entrypoint: "approve", calldata: [POOL, ...Object.values(cairo.uint256(poolFee)).map(String)] });
  log(`batching approve(pool, ${fmt(poolFee)} STRK) into the same transaction`);
}
calls.push(callAndProof.call);

const proofDetails = callAndProof.proof.proofFacts?.length
  ? { proofFacts: callAndProof.proof.proofFacts, proof: callAndProof.proof.data }
  : {};
let est;
try {
  est = await account.estimateInvokeFee(calls, { tip: 0n, ...proofDetails });
} catch (e) {
  const m = String(e.message ?? e);
  const inner = m.match(/execution_error"?:\s*"?([^"}]{0,400})/)?.[1] ?? m.slice(0, 400);
  log("estimateInvokeFee FAILED:", inner);
  throw new Error("aborting before submit: " + inner);
}
const ceiling = boundsCeiling(est.resourceBounds);
log(`estimate: overall ${fmt(BigInt(est.overall_fee))} STRK, bounds ceiling ${fmt(ceiling)} STRK`);
log(`budget: balance ${fmt(bal0)} STRK; need ceiling ${fmt(ceiling)} to clear validation, then pool fee ${fmt(poolFee)} + gas`);
if (bal0 < ceiling) {
  throw new Error(`SHORTFALL: balance ${fmt(bal0)} STRK < required resource-bounds ceiling ${fmt(ceiling)} STRK (short by ${fmt(ceiling - bal0)} STRK)`);
}
if (DRY_RUN) { log("DRY_RUN: stopping before submit"); process.exit(0); }

// ── submit ─────────────────────────────────────────────────────────────────
const tx = await account.execute(calls, { tip: 0n, ...proofDetails });
log("tx", tx.transaction_hash);
const rc = await provider.waitForTransaction(tx.transaction_hash, WAIT_OPTIONS);
log("result:", rc.execution_status, "block", rc.block_number, "fee", fmt(BigInt(rc.actual_fee?.amount ?? 0)), "STRK");
recordTransaction({
  [`spend_${SHAPE}`]: {
    hash: tx.transaction_hash, block: rc.block_number, fee_fri: rc.actual_fee?.amount,
    spent_note_id: String(spend.id), spent_amount: spend.amount.toString(),
    proving_block: provingBlockId, pool_class_hash: classHash, batched_approve: calls.length > 1,
  },
});
if (!rc.isSuccess?.()) { console.log(JSON.stringify(rc, null, 1).slice(0, 3000)); throw new Error("transaction reverted"); }

const NAMES = ["ViewingKeySet", "Withdrawal", "Deposit", "OpenNoteCreated", "EncNoteCreated", "NoteUsed"];
const sel = new Map(NAMES.map((n) => [BigInt(hash.getSelectorFromName(n)), n]));
log("pool events:");
for (const e of rc.events ?? []) {
  if (BigInt(e.from_address) !== BigInt(POOL)) continue;
  log("   ", (sel.get(BigInt(e.keys[0])) ?? "?").padEnd(15), `keys=${JSON.stringify(e.keys)} data=${JSON.stringify(e.data)}`);
}
log("post balance", fmt(await balance()), "STRK");
