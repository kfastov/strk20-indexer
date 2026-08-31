#!/usr/bin/env node
// 02 — deploy the account created by step 01. One DEPLOY_ACCOUNT v3 transaction,
// paid in STRK by the account itself, which is why it must be funded first.
//
// Idempotent: if the account is already on chain this exits 0 without spending.
// If the balance is short it fails before submitting and tells you the gap.
import { CallData } from "starknet";
import { CFG, loadKeystore, providerFor, accountFor, guardChain, balanceOf, boundsCeiling, withMargin, writeState, log, step, fail, fmt, WAIT_OPTIONS } from "./lib.mjs";

const cfg = CFG();
const ks = loadKeystore(cfg);
const provider = providerFor(cfg);

step("chain");
const { chainId, spec } = await guardChain(provider, cfg);
log(`rpc ${cfg.rpc}  chain ${chainId}  spec ${spec}  head ${await provider.getBlockNumber()}`);
log(`account ${ks.account_address}`);

// ── already deployed? ────────────────────────────────────────────────────────
try {
  const nonce = await provider.getNonceForAddress(ks.account_address, "latest");
  log(`this account is already deployed (nonce ${nonce}). Nothing to do.`);
  const bal = await balanceOf(provider, cfg, ks.account_address);
  log(`balance ${fmt(bal)} STRK`);
  log(`next: node 03-shield.mjs`);
  process.exit(0);
} catch {
  // not deployed yet — carry on
}

// ── funded? ──────────────────────────────────────────────────────────────────
step("balance");
const balance = await balanceOf(provider, cfg, ks.account_address);
log(`STRK balance ${fmt(balance)}`);
if (balance === 0n) {
  fail(
    `this account has no STRK, so it cannot pay for its own deployment.\n` +
      `  Send STRK to ${ks.account_address}\n` +
      `  Token: ${cfg.strk}\n` +
      `  Re-run 01-create-account.mjs to see the recommended amount (it does not\n` +
      `  regenerate an existing account).`
  );
}

// ── estimate ─────────────────────────────────────────────────────────────────
const account = accountFor(cfg, ks, provider);
const payload = {
  classHash: ks.account_class,
  constructorCalldata: CallData.compile({ publicKey: ks.account_public_key }),
  addressSalt: ks.account_salt,
  contractAddress: ks.account_address,
};

step("fee estimate");
let est;
try {
  est = await account.estimateAccountDeployFee(payload, { tip: 0n, skipValidate: true });
} catch (e) {
  fail(`could not estimate the deployment fee.\n  ${String(e.message ?? e).slice(0, 400)}\n  Nothing was submitted.`);
}
const bounds = withMargin(est.resourceBounds, cfg.feeMarginPercent);
const ceiling = boundsCeiling(bounds);
log(`estimate ${fmt(BigInt(est.overall_fee))} STRK; resource-bounds ceiling ${fmt(ceiling)} STRK`);
if (balance < ceiling) {
  fail(
    `INSUFFICIENT BALANCE — nothing was submitted, nothing was spent.\n` +
      `  account ${ks.account_address}\n` +
      `  has     ${fmt(balance)} STRK\n` +
      `  needs   ${fmt(ceiling)} STRK  (short by ${fmt(ceiling - balance)} STRK)\n\n` +
      `  Send at least ${fmt(ceiling - balance)} more STRK and re-run.`
  );
}
log(`headroom ${fmt(balance - ceiling)} STRK  \x1b[32mOK\x1b[0m`);

if (cfg.dryRun) {
  step("DRY RUN");
  log(`DRY_RUN=1 — the account is fundable and deployable. Nothing was sent.`);
  log(`Set DRY_RUN=0 to deploy for real.`);
  process.exit(0);
}

// ── deploy ───────────────────────────────────────────────────────────────────
step("deploying");
const res = await account.deployAccount(payload, { tip: 0n, resourceBounds: bounds });
log(`tx hash  ${res.transaction_hash}`);
log(`explorer https://voyager.online/tx/${res.transaction_hash}`);
const rc = await provider.waitForTransaction(res.transaction_hash, WAIT_OPTIONS);
log(`status   ${rc.execution_status} in block ${rc.block_number}`);
const paid = BigInt(rc.actual_fee?.amount ?? 0);
log(`gas paid ${fmt(paid)} STRK`);
if (!rc.isSuccess?.()) fail(`deployment REVERTED. Gas was still charged.`);

writeState(cfg, { deploy: { hash: res.transaction_hash, block: rc.block_number, fee_fri: rc.actual_fee?.amount } });
log(`remaining balance ${fmt(await balanceOf(provider, cfg, ks.account_address))} STRK`);
step("next");
console.log(`  node 03-shield.mjs      (do a DRY_RUN=1 pass first)\n`);
