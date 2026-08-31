#!/usr/bin/env node
// 04 — private transfer: spend an existing note and create a new one.
// Emits NoteUsed (with the spent note's nullifier) + EncNoteCreated.
//
// The value never becomes public: no Deposit, no Withdrawal. Only the pool fee
// and gas leave your public balance.
//
// STRK20_TRANSFER_TO blank => transfer to yourself. That is a real spend and
// the cheapest way to exercise the nullifier path.
import {
  CFG, loadKeystore, providerFor, accountFor, guardChain, printPool, balanceOf,
  makeTransfers, buildAndProve, callsWithApprove, estimateAndCheck, submit,
  reportPoolEvents, spendableNotes, guardStep, writeState, dryRunStop,
  log, step, fail, fmt, toWei, env,
} from "./lib.mjs";

const cfg = CFG();
const ks = loadKeystore(cfg);
guardStep(cfg, "transfer");

const recipient = process.env.STRK20_TRANSFER_TO?.trim() || ks.account_address;
const provider = providerFor(cfg);

step("chain");
const { chainId } = await guardChain(provider, cfg);
log(`chain ${chainId}  account ${ks.account_address}`);
try {
  await provider.getNonceForAddress(ks.account_address, "latest");
} catch {
  fail(`this account is not deployed yet.\n  run:  node 02-deploy-account.mjs`);
}
const pool = await printPool(provider, cfg);
const account = accountFor(cfg, ks, provider);
const transfers = await makeTransfers(cfg, ks, account);

const notes = await spendableNotes(transfers, cfg);
const total = notes.reduce((a, n) => a + n.amount, 0n);
const amount = process.env.STRK20_TRANSFER_STRK?.trim() ? toWei(process.env.STRK20_TRANSFER_STRK, "STRK20_TRANSFER_STRK") : total;

step("plan");
log(`transfer  ${fmt(amount)} STRK`);
log(`to        ${recipient}${recipient === ks.account_address ? "  (yourself)" : ""}`);
log(`from      ${notes.length} note(s) totalling ${fmt(total)} STRK`);
log(`pool fee  ${fmt(pool.fee)} STRK from your PUBLIC balance (charged on every apply_actions)`);
log(`public    ${fmt(await balanceOf(provider, cfg, ks.account_address))} STRK`);
if (amount > total) {
  fail(`you are trying to transfer ${fmt(amount)} STRK but only hold ${fmt(total)} STRK shielded.\n` + `  Lower STRK20_TRANSFER_STRK, or leave it blank to move everything.`);
}
if (recipient !== ks.account_address) {
  log(`\x1b[33mnote\x1b[0m the recipient must already be registered in the pool, or the`);
  log(`     transfer cannot resolve a channel for them and will fail while building.`);
}

const chain = transfers
  .build({ autoRegister: true, autoSetup: true, autoDiscover: { notes: "refresh", channels: "refresh" }, autoSelectNotes: "naive" })
  .surplusTo(ks.account_address)
  .with(cfg.strk, (t) => { t.transfer({ recipient, amount }); });

const { callAndProof, details } = await buildAndProve(transfers, chain, provider);
// A transfer carries no Deposit, so no screening attestation is minted or needed.
const calls = await callsWithApprove(provider, cfg, ks.account_address, callAndProof.call, pool.fee);
const { bounds } = await estimateAndCheck(account, provider, cfg, calls, details, {
  spentDuringExecution: pool.fee,
  label: "transfer",
});

if (dryRunStop(cfg, "the transfer is built, proved and affordable")) process.exit(0);

const { tx, receipt } = await submit(account, provider, cfg, calls, details, bounds);
const events = reportPoolEvents(receipt, cfg);
const names = new Set(events.map((e) => e.name));
for (const want of ["NoteUsed", "EncNoteCreated"]) {
  if (!names.has(want)) log(`\x1b[33mnote\x1b[0m expected a ${want} event and did not see one`);
}
const used = events.find((e) => e.name === "NoteUsed");
const made = events.find((e) => e.name === "EncNoteCreated");
writeState(cfg, {
  transfer: {
    hash: tx.transaction_hash,
    block: receipt.block_number,
    fee_fri: receipt.actual_fee?.amount,
    amount: amount.toString(),
    recipient,
    pool_class_hash: pool.classHash,
    nullifier: used?.keys?.[1],
    new_note_id: made?.keys?.[1],
    events: [...names],
  },
});

step("done");
log(`nullifier of the spent note ${used?.keys?.[1] ?? "(not seen)"}`);
log(`new note id                 ${made?.keys?.[1] ?? "(not seen)"}`);
console.log(`\n  next: node 05-withdraw.mjs\n`);
