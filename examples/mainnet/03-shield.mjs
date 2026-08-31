#!/usr/bin/env node
// 03 — shield: register the viewing key and move STRK into the pool, in ONE
// apply_actions transaction (the SDK batches SetViewingKey + Deposit +
// CreateEncNote when autoRegister/autoSetup are on).
//
// Emits ViewingKeySet + Deposit + EncNoteCreated. The EncNoteCreated is the
// note that strk20-sync will later discover with the viewing key from step 01.
//
// Cost, all from your PUBLIC balance:
//   deposit amount + pool fee (read live) + gas.
// The pool fee is NOT taken out of the note: the note is worth the full deposit.
import {
  CFG, loadKeystore, providerFor, accountFor, guardChain, printPool, balanceOf,
  makeTransfers, buildAndProve, callsWithApprove, estimateAndCheck, submit,
  reportPoolEvents, spendableNotes, guardStep, writeState, dryRunStop,
  log, step, fail, fmt, toWei, env,
} from "./lib.mjs";

const cfg = CFG();
const ks = loadKeystore(cfg);
guardStep(cfg, "shield");

const deposit = toWei(env("STRK20_DEPOSIT_STRK", "2"), "STRK20_DEPOSIT_STRK");
if (deposit <= 0n) fail("STRK20_DEPOSIT_STRK must be greater than 0.");

const provider = providerFor(cfg);
step("chain");
const { chainId, spec } = await guardChain(provider, cfg);
log(`rpc ${cfg.rpc}  chain ${chainId}  spec ${spec}`);
log(`account ${ks.account_address}`);
try {
  await provider.getNonceForAddress(ks.account_address, "latest");
} catch {
  fail(`this account is not deployed yet.\n  run:  node 02-deploy-account.mjs`);
}

const pool = await printPool(provider, cfg);
const account = accountFor(cfg, ks, provider);

step("plan");
log(`deposit         ${fmt(deposit)} STRK  -> becomes a shielded note worth exactly this`);
log(`pool fee        ${fmt(pool.fee)} STRK  -> charged from your public balance (collect_fee)`);
log(`allowance need  ${fmt(deposit + pool.fee)} STRK  -> the pool transferFrom's BOTH`);
log(`public balance  ${fmt(await balanceOf(provider, cfg, ks.account_address))} STRK`);
if (pool.screeningEnforced) log(`screening is ENFORCED: the prover must mint an attestation for this deposit`);

const transfers = await makeTransfers(cfg, ks, account);
// A note already present is not an error, but say so — a second shield creates
// a second note rather than topping the first one up.
await spendableNotes(transfers, cfg, { required: false });

const chain = transfers
  .build({ autoRegister: true, autoSetup: true, autoDiscover: { notes: "refresh", channels: "refresh" }, autoSelectNotes: "naive" })
  .surplusTo(ks.account_address)
  .with(cfg.strk, (t) => { t.deposit({ amount: deposit }); });

const { callAndProof, details } = await buildAndProve(transfers, chain, provider);
if (pool.screeningEnforced && !callAndProof.proof.additionalData?.signature) {
  fail(
    `the pool enforces deposit screening but the prover returned no attestation.\n` +
      `  Submitting would revert and still cost gas, so this stops here.\n` +
      `  This usually means the screening provider rejected the depositor address.`
  );
}

const calls = await callsWithApprove(provider, cfg, ks.account_address, callAndProof.call, deposit + pool.fee);
const { bounds } = await estimateAndCheck(account, provider, cfg, calls, details, {
  spentDuringExecution: deposit + pool.fee,
  label: "shield",
});

if (dryRunStop(cfg, "the deposit is built, proved, screened and affordable")) process.exit(0);

const { tx, receipt } = await submit(account, provider, cfg, calls, details, bounds);
const events = reportPoolEvents(receipt, cfg);
const names = new Set(events.map((e) => e.name));
for (const want of ["ViewingKeySet", "Deposit", "EncNoteCreated"]) {
  if (!names.has(want)) log(`\x1b[33mnote\x1b[0m expected a ${want} event and did not see one`);
}
const note = events.find((e) => e.name === "EncNoteCreated");
writeState(cfg, {
  shield: {
    hash: tx.transaction_hash,
    block: receipt.block_number,
    fee_fri: receipt.actual_fee?.amount,
    deposit: deposit.toString(),
    pool_fee: pool.fee.toString(),
    pool_class_hash: pool.classHash,
    note_id: note?.keys?.[1],
    events: [...names],
  },
});

step("done");
log(`shielded note id ${note?.keys?.[1] ?? "(not found in events)"}`);
log(`public balance now ${fmt(await balanceOf(provider, cfg, ks.account_address))} STRK`);
console.log(`
  Wait a few blocks, then:
    node 04-transfer.mjs      (private note-to-note spend)
    ./06-discover.sh          (prove your own indexer can find this note)
`);
