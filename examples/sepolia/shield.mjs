// Run 1 — register the viewing key and shield a STRK deposit on the Sepolia
// STRK20 privacy pool, headless, in ONE apply_actions transaction. TESTNET ONLY:
// the script aborts unless starknet_chainId == SN_SEPOLIA.
//
// Flow (mirrors upstream demo/src/hooks/useTransactionBuilder.ts, direct-execution branch):
//   1. ERC20 approve(pool, deposit + poolFee) — collect_fee charges the CALLER's
//      public balance before applying actions, so the fee must be inside the allowance
//   2. transfers.build({autoRegister, autoSetup, …}).with(STRK).deposit({amount})
//   3. createProofInvocation at head-9 -> executeWithInvocation (hosted prover)
//   4. account.execute(call, { tip: 0n, proof, proofFacts })
//
// Env: see .env.example. DRY_RUN=1 stops before every submit.
import fs from "node:fs";
import { RpcProvider, Account, constants, cairo, TransactionFinalityStatus, hash } from "starknet";
import { createPrivateTransfers, ProvingServiceProofProvider, IndexerDiscoveryProvider } from "@starkware-libs/starknet-privacy-sdk";
import {
  RPC, POOL, STRK, PROVER_URL, DISCOVERY_URL, SEPOLIA_CHAIN_ID, ACCOUNT_NAME, DRY_RUN,
  PROVING_BLOCK_DEPTH, loadAccount, deriveViewingKey, recordTransaction, keyFilePath,
  outFile, fmt, log, boundsCeiling,
} from "./lib.mjs";

const DEPOSIT_STRK = process.env.DEPOSIT_STRK ?? "3";
const depositAmount = BigInt(Math.round(Number(DEPOSIT_STRK) * 1e6)) * 10n ** 12n;

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

const view = (contractAddress, entrypoint, calldata = []) => provider.callContract({ contractAddress, entrypoint, calldata }, "latest");
const u256 = async (c, e, d) => { const [lo, hi] = await view(c, e, d); return BigInt(lo) + (BigInt(hi) << 128n); };
const balance = () => u256(STRK, "balance_of", [a.address]);
const allowance = () => u256(STRK, "allowance", [a.address, POOL]);

const chainId = await provider.getChainId();
if (chainId !== constants.StarknetChainId.SN_SEPOLIA) throw new Error(`REFUSING: chain id ${chainId} is not SN_SEPOLIA`);

const poolFee = BigInt((await view(POOL, "get_fee_amount"))[0]);
const needAllowance = depositAmount + poolFee;
log(`chain SN_SEPOLIA, head ${await provider.getBlockNumber()}`);
log(`account ${a.address}  balance ${fmt(await balance())} STRK`);
log(`deposit ${fmt(depositAmount)} STRK, pool fee ${fmt(poolFee)} STRK -> allowance needed ${fmt(needAllowance)} STRK`);

// ── 1. approve ─────────────────────────────────────────────────────────────
let have = await allowance();
log(`current allowance to pool: ${fmt(have)} STRK`);
if (have < needAllowance) {
  const approveCall = {
    contractAddress: STRK,
    entrypoint: "approve",
    calldata: [POOL, ...Object.values(cairo.uint256(needAllowance)).map(String)],
  };
  const est = await account.estimateInvokeFee([approveCall], { tip: 0n });
  log(`approve estimate: overall ${fmt(BigInt(est.overall_fee))} STRK, ceiling ${fmt(boundsCeiling(est.resourceBounds))} STRK`);
  if (DRY_RUN) { log("DRY_RUN: stopping before approve"); process.exit(0); }
  const tx = await account.execute([approveCall], { tip: 0n });
  log("approve tx", tx.transaction_hash);
  const rc = await provider.waitForTransaction(tx.transaction_hash, WAIT_OPTIONS);
  log("approve", rc.execution_status, "fee", fmt(BigInt(rc.actual_fee?.amount ?? 0)), "block", rc.block_number);
  recordTransaction({ approve: { hash: tx.transaction_hash, block: rc.block_number, fee_fri: rc.actual_fee?.amount, amount: needAllowance.toString() } });
  have = await allowance();
  log("allowance now", fmt(have), "STRK");
}

// ── 2. SDK wiring ──────────────────────────────────────────────────────────
const discoveryProvider = new IndexerDiscoveryProvider(DISCOVERY_URL, POOL); // no options => OHTTP off
log("discovery healthy:", await discoveryProvider.isHealthy());

const transfers = createPrivateTransfers({
  account,
  viewingKeyProvider: { getViewingKey: async () => viewingKey },
  provingProvider: new ProvingServiceProofProvider(PROVER_URL, constants.StarknetChainId.SN_SEPOLIA, { requestTimeoutMs: 180_000 }),
  discoveryProvider,
  poolContractAddress: POOL,
});

// ── 3. build + prove ───────────────────────────────────────────────────────
const chain = transfers
  .build({ autoRegister: true, autoSetup: true, autoDiscover: { notes: "refresh", channels: "refresh" }, autoSelectNotes: "naive" })
  .surplusTo(a.address)
  .with(STRK, (t) => { t.deposit({ amount: depositAmount }); });

const head = await provider.getBlockNumber();
const provingBlockId = head - PROVING_BLOCK_DEPTH;
log(`head ${head}; building proof invocation at block ${provingBlockId} (head-${PROVING_BLOCK_DEPTH})`);
const invocation = await chain.createProofInvocation({ provingBlockId });
log("invocation built; warnings:", JSON.stringify(invocation.warnings));

const t0 = Date.now();
const { callAndProof, warnings } = await transfers.executeWithInvocation(invocation, provingBlockId);
log(`proved in ${((Date.now() - t0) / 1000).toFixed(1)}s; proof ${Math.round((callAndProof.proof.data?.length ?? 0) * 0.75)} bytes, ${callAndProof.proof.proofFacts.length} proof facts`);
log("screening attestation present:", Boolean(callAndProof.proof.additionalData?.signature));
log("apply_actions calldata felts:", callAndProof.call.calldata.length, "| warnings:", JSON.stringify(warnings));
fs.writeFileSync(
  outFile("last-callandproof.json"),
  JSON.stringify({ call: callAndProof.call, proofFacts: callAndProof.proof.proofFacts, output: callAndProof.proof.output, additionalData: callAndProof.proof.additionalData, provingBlockId }, null, 1),
);

// ── 4. estimate + submit ───────────────────────────────────────────────────
const proofDetails = callAndProof.proof.proofFacts?.length
  ? { proofFacts: callAndProof.proof.proofFacts, proof: callAndProof.proof.data }
  : {};
let est;
try {
  est = await account.estimateInvokeFee([callAndProof.call], { tip: 0n, ...proofDetails });
  log(`apply_actions estimate: overall ${fmt(BigInt(est.overall_fee))} STRK, bounds ceiling ${fmt(boundsCeiling(est.resourceBounds))} STRK`);
} catch (e) {
  const m = String(e.message ?? e);
  const inner = m.match(/execution_error"?:\s*"?([^"}]{0,300})/)?.[1] ?? m.slice(0, 300);
  log("estimateInvokeFee FAILED:", inner);
  throw new Error("aborting before submit: fee estimation failed -> " + inner);
}
const bal = await balance();
const ceiling = boundsCeiling(est.resourceBounds);
log(`budget check: balance ${fmt(bal)} STRK vs ceiling ${fmt(ceiling)} + deposit ${fmt(depositAmount)} + poolFee ${fmt(poolFee)} = ${fmt(ceiling + depositAmount + poolFee)}`);
if (bal < ceiling) throw new Error(`SHORTFALL: balance ${fmt(bal)} < required resource-bounds ceiling ${fmt(ceiling)} STRK`);
if (DRY_RUN) { log("DRY_RUN: stopping before apply_actions submit"); process.exit(0); }

const tx = await account.execute(callAndProof.call, { tip: 0n, ...proofDetails });
log("apply_actions tx", tx.transaction_hash);
const rc = await provider.waitForTransaction(tx.transaction_hash, WAIT_OPTIONS);
log("result:", rc.execution_status, "block", rc.block_number, "fee", fmt(BigInt(rc.actual_fee?.amount ?? 0)), "STRK");
recordTransaction({
  apply_actions: {
    hash: tx.transaction_hash, block: rc.block_number, fee_fri: rc.actual_fee?.amount,
    deposit_amount: depositAmount.toString(), pool_fee: poolFee.toString(), proving_block: provingBlockId,
  },
});
if (!rc.isSuccess?.()) { console.log(JSON.stringify(rc, null, 1).slice(0, 3000)); throw new Error("transaction reverted"); }

const NAMES = ["ViewingKeySet", "Withdrawal", "Deposit", "OpenNoteCreated", "EncNoteCreated", "NoteUsed"];
const sel = new Map(NAMES.map((n) => [BigInt(hash.getSelectorFromName(n)), n]));
log("pool events:");
for (const e of rc.events ?? []) {
  if (BigInt(e.from_address) !== BigInt(POOL)) continue;
  log("   ", (sel.get(BigInt(e.keys[0])) ?? "?").padEnd(16), e.keys[0]);
}
log("post balance", fmt(await balance()), "STRK");
log("key file:", keyFilePath());
