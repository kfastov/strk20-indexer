#!/usr/bin/env node
// 05 — withdraw: move shielded value back out to a public address.
// Emits NoteUsed + Withdrawal (+ EncNoteCreated when there is change).
//
// This is the step that returns your deposit. Note the ordering cost: the pool
// fee and gas are charged BEFORE the withdrawn value lands, so the account still
// needs to clear the resource-bounds ceiling on its own.
//
// STRK20_WITHDRAW_STRK blank => withdraw everything.
// STRK20_WITHDRAW_TO    blank => withdraw to your own account.
import {
  CFG, loadKeystore, providerFor, accountFor, guardChain, printPool, balanceOf,
  makeTransfers, buildAndProve, callsWithApprove, estimateAndCheck, submit,
  reportPoolEvents, spendableNotes, guardStep, writeState, dryRunStop,
  log, step, fail, fmt, toWei,
} from "./lib.mjs";

const cfg = CFG();
const ks = loadKeystore(cfg);
guardStep(cfg, "withdraw");

const recipient = process.env.STRK20_WITHDRAW_TO?.trim() || ks.account_address;
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
const amount = process.env.STRK20_WITHDRAW_STRK?.trim() ? toWei(process.env.STRK20_WITHDRAW_STRK, "STRK20_WITHDRAW_STRK") : total;

step("plan");
log(`withdraw  ${fmt(amount)} STRK of ${fmt(total)} STRK shielded`);
log(`to        ${recipient}${recipient === ks.account_address ? "  (yourself)" : ""}`);
log(`pool fee  ${fmt(pool.fee)} STRK from your PUBLIC balance, charged before the value arrives`);
const before = await balanceOf(provider, cfg, ks.account_address);
log(`public    ${fmt(before)} STRK`);
if (amount > total) {
  fail(`you are trying to withdraw ${fmt(amount)} STRK but only hold ${fmt(total)} STRK shielded.\n` + `  Lower STRK20_WITHDRAW_STRK, or leave it blank to withdraw everything.`);
}
if (amount < total) log(`the remaining ${fmt(total - amount)} STRK comes back as a change note (a second EncNoteCreated)`);

const chain = transfers
  .build({ autoRegister: true, autoSetup: true, autoDiscover: { notes: "refresh", channels: "refresh" }, autoSelectNotes: "naive" })
  .surplusTo(ks.account_address)
  .with(cfg.strk, (t) => { t.withdraw({ recipient, amount }); });

const { callAndProof, details } = await buildAndProve(transfers, chain, provider);
const calls = await callsWithApprove(provider, cfg, ks.account_address, callAndProof.call, pool.fee);
const { bounds } = await estimateAndCheck(account, provider, cfg, calls, details, {
  spentDuringExecution: pool.fee,
  label: "withdraw",
});

if (dryRunStop(cfg, "the withdrawal is built, proved and affordable")) process.exit(0);

const { tx, receipt } = await submit(account, provider, cfg, calls, details, bounds);
const events = reportPoolEvents(receipt, cfg);
const names = new Set(events.map((e) => e.name));
for (const want of ["NoteUsed", "Withdrawal"]) {
  if (!names.has(want)) log(`\x1b[33mnote\x1b[0m expected a ${want} event and did not see one`);
}
const used = events.find((e) => e.name === "NoteUsed");
writeState(cfg, {
  withdraw: {
    hash: tx.transaction_hash,
    block: receipt.block_number,
    fee_fri: receipt.actual_fee?.amount,
    amount: amount.toString(),
    recipient,
    pool_class_hash: pool.classHash,
    nullifier: used?.keys?.[1],
    events: [...names],
  },
});

const after = await balanceOf(provider, cfg, ks.account_address);
step("done");
log(`public balance ${fmt(before)} -> ${fmt(after)} STRK  (net ${fmt(after - before)})`);
log(`nullifier of the spent note ${used?.keys?.[1] ?? "(not seen)"}`);
console.log(`
  The lifecycle is complete. Your indexer should now see, for this address:
    a registration, a deposit, at least two note creations, and two spends.
  Close the loop:  ./06-discover.sh
`);
