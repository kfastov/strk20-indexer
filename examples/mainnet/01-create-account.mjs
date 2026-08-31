#!/usr/bin/env node
// 01 — create a fresh mainnet account, LOCALLY. Spends nothing, sends nothing.
//
// Produces a keypair, computes the counterfactual address the account will have
// once deployed, derives the STRK20 viewing key from it, and writes both to the
// keystore directory. Then it reads the live pool configuration and tells you
// exactly how much STRK to send the address before step 02.
//
// The only network calls are reads: chain id, spec version, the pool's own
// views, and the prover/discovery health checks.
import fs from "node:fs";
import { ec, hash, stark, constants, CallData } from "starknet";
import { CFG, keystorePaths, providerFor, guardChain, printPool, deriveViewingKey, balanceOf, log, step, fail, fmt, mask, toWei, assertAddressAgrees, STRK } from "./lib.mjs";

const cfg = CFG();
const p = keystorePaths(cfg.keystore);

if (fs.existsSync(p.account) && !cfg.force) {
  const existing = JSON.parse(fs.readFileSync(p.account, "utf8"));
  log(`an account already exists at ${p.account}`);
  log(`address ${existing.account_address}`);
  log(`Refusing to overwrite it — that would orphan any funds and notes it owns.`);
  log(`Delete the keystore directory yourself, or set FORCE=1, if you really mean to.`);
  process.exit(0);
}

const provider = providerFor(cfg);
step("chain");
const { chainId, spec } = await guardChain(provider, cfg);
log(`rpc          ${cfg.rpc}`);
log(`chain id     ${chainId}  (SN_MAIN)`);
log(`spec version ${spec}`);
log(`head block   ${await provider.getBlockNumber()}`);

const pool = await printPool(provider, cfg);

step("privacy services");
const probes = [
  ["prover   ", cfg.prover, async (u) => {
    const r = await fetch(u, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "starknet_specVersion", params: [] }) });
    const j = await r.json();
    return j.result ? `OK (starknet spec ${j.result})` : `unexpected: ${JSON.stringify(j).slice(0, 120)}`;
  }],
  ["discovery", cfg.discovery, async (u) => {
    const r = await fetch(u + "/health");
    const j = await r.json();
    return j.status === "OK" ? `OK (head ${j.chain_head?.block_number}, lag ${j.lag_secs}s)` : JSON.stringify(j).slice(0, 120);
  }],
];
for (const [name, url, probe] of probes) {
  try { log(`${name} ${url} -> ${await probe(url)}`); }
  catch (e) { log(`${name} ${url} -> \x1b[31mUNREACHABLE\x1b[0m ${String(e.message ?? e).slice(0, 120)}`); }
}

// ── the keypair (offline) ────────────────────────────────────────────────────
step("generating account (offline)");
const privateKey = stark.randomAddress();
const publicKey = ec.starkCurve.getStarkKey(privateKey);
const salt = publicKey; // conventional: salt = pubkey, as sncast/starkli do
const constructorCalldata = CallData.compile({ publicKey });
const address = hash.calculateContractAddressFromHash(salt, cfg.accountClass, constructorCalldata, 0);

// Independently re-derive the address before we ever tell anyone to fund it.
assertAddressAgrees(address, salt, cfg.accountClass, constructorCalldata);
log(`address derivation cross-checked by an independent implementation ✓`);

// verify the account class really is declared, or step 02 cannot possibly work
try {
  await provider.getClass(cfg.accountClass, "latest");
  log(`account class ${cfg.accountClass} is declared on mainnet ✓`);
} catch (e) {
  fail(`account class ${cfg.accountClass} is NOT declared on mainnet.\n  ${String(e.message ?? e).slice(0, 200)}\n  Set STRK20_ACCOUNT_CLASS to a class that is.`);
}

const viewingKey = deriveViewingKey(privateKey, chainId, cfg.pool);

fs.mkdirSync(p.dir, { recursive: true, mode: 0o700 });
fs.chmodSync(p.dir, 0o700);
const record = {
  _comment:
    "STRK20 mainnet key material. SECRET. The viewing key is derived deterministically " +
    "from account_private_key for THIS chain id and THIS pool address; changing either " +
    "changes the key. Keep this file; without it the notes are unrecoverable.",
  network: "mainnet",
  chain_id: chainId,
  rpc: cfg.rpc,
  pool_address: cfg.pool,
  strk_token: cfg.strk,
  account_class: cfg.accountClass,
  account_address: address,
  account_public_key: publicKey,
  account_private_key: privateKey,
  account_salt: salt,
  viewing_key: "0x" + viewingKey.toString(16),
  derivation: {
    message: `${chainId}:${cfg.pool}`,
    scheme: "starknetKeccak(message) -> stark ECDSA sign -> poseidon([r,s]) mod n, canonical lower half",
  },
  created_at: new Date().toISOString(),
};
fs.writeFileSync(p.account, JSON.stringify(record, null, 2) + "\n", { mode: 0o600 });
fs.chmodSync(p.account, 0o600);
fs.writeFileSync(p.viewingKey, "0x" + viewingKey.toString(16) + "\n", { mode: 0o600 });
fs.chmodSync(p.viewingKey, 0o600);

log(`wrote ${p.account}      (mode 0600)`);
log(`wrote ${p.viewingKey}  (mode 0600)  <- this is what strk20-sync --key-file reads`);
log(`private key  ${mask(privateKey)}   [not printed]`);
log(`viewing key  ${mask("0x" + viewingKey.toString(16))}   [not printed]`);

// ── funding instructions ─────────────────────────────────────────────────────
// Cost model, all read live except the gas figures, which are OBSERVED ranges
// from recent mainnet pool transactions (see README).
const deposit = toWei(process.env.STRK20_DEPOSIT_STRK ?? "2", "STRK20_DEPOSIT_STRK");
const DEPLOY_GAS = STRK / 4n;             // 0.25 STRK; live mainnet DEPLOY_ACCOUNT estimate was 0.179645
const SHIELD_CEILING = 17n * STRK;        // worst mainnet shield ceiling observed (7.5-17.1)
const SPEND_CEILING = 10n * STRK;         // worst mainnet spend ceiling observed (6.6-10.1)
const POOL_TX_GAS = 4n * STRK;            // generous per-tx actual gas (observed 2.9-6.9)
const max = (a, b) => (a > b ? a : b);

// Walk the lifecycle BACKWARDS: each step must leave enough behind for the next
// one to clear its own resource-bounds ceiling at validation time. Withdraw
// returns the note's value, but only after its ceiling has been covered.
const beforeWithdraw = max(SPEND_CEILING, pool.fee + POOL_TX_GAS);
const beforeTransfer = max(SPEND_CEILING, beforeWithdraw + pool.fee + POOL_TX_GAS);
const beforeShield = max(SHIELD_CEILING, beforeTransfer + deposit + pool.fee + POOL_TX_GAS);
const recommended = beforeShield + DEPLOY_GAS;
const minimum = DEPLOY_GAS + max(SHIELD_CEILING, deposit + pool.fee + POOL_TX_GAS);
const netCost = pool.fee * 3n + POOL_TX_GAS * 3n + DEPLOY_GAS; // deposit comes back at step 05

step("FUND THIS ADDRESS");
console.log(`
  \x1b[1m${address}\x1b[0m

  Send \x1b[1m${fmt(recommended)} STRK\x1b[0m to run the whole lifecycle (steps 02-05).
  Send at least ${fmt(minimum)} STRK if you only want to get through the shield (steps 02-03).

  Where it goes, with a ${fmt(deposit)} STRK deposit and the pool's live ${fmt(pool.fee)} STRK fee:
    02 deploy account       ~${fmt(DEPLOY_GAS)} STRK gas
    03 shield               ${fmt(deposit)} deposit + ${fmt(pool.fee)} pool fee + ~${fmt(POOL_TX_GAS)} gas
    04 transfer             ${fmt(pool.fee)} pool fee + ~${fmt(POOL_TX_GAS)} gas
    05 withdraw             ${fmt(pool.fee)} pool fee + ~${fmt(POOL_TX_GAS)} gas, and ${fmt(deposit)} comes back

  Expected NET cost of the full lifecycle: about \x1b[1m${fmt(netCost)} STRK\x1b[0m. The deposit
  returns to you at step 05; what you actually spend is 3 pool fees plus gas.
  Raising or lowering the deposit changes how much you must HOLD, not what it costs.

  The pool fee is charged from your PUBLIC balance on every pool transaction, so
  the shielded note is worth the full deposit. Each step also needs its
  resource-bounds ceiling covered at validation time (up to ~${fmt(SHIELD_CEILING)} STRK for the
  shield) even though the bill is far smaller — that headroom is included above.

  Use ONLY the STRK token ${cfg.strk}
  Do not send ETH: it cannot pay for any of this.
`);

const bal = await balanceOf(provider, cfg, address);
log(`current balance of that address: ${fmt(bal)} STRK`);
log(bal > 0n ? "already funded — you can go straight to 02-deploy-account.mjs" : "unfunded, as expected for a brand new address");
step("next");
console.log(`  1. send the STRK above to the address above
  2. node 02-deploy-account.mjs
`);
