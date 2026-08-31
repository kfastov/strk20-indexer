// Shared helpers for the STRK20 mainnet lifecycle scripts.
//
// Everything here is driven by environment variables. Nothing in examples/
// reads the repository's gitignored `data/` tree.
//
// Safety invariants enforced for every script that can submit a transaction:
//   - the RPC must report chain id SN_MAIN (guardChain)
//   - DRY_RUN=1 stops before any submission
//   - the account balance is checked against the resource-bounds ceiling BEFORE
//     submitting, so a short balance costs nothing instead of a failed tx
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { RpcProvider, Account, constants, cairo, hash, ec, num, TransactionFinalityStatus } from "starknet";
import { MAX_VIEWING_KEY } from "@starkware-libs/starknet-privacy-sdk";

// ── env ──────────────────────────────────────────────────────────────────────
export function env(name, fallback) {
  const v = process.env[name];
  if (v === undefined || v === "") {
    if (fallback !== undefined) return fallback;
    fail(`missing required environment variable ${name}\n` + `  copy .env.example to .env, edit it, then:  set -a; . ./.env; set +a`);
  }
  return v;
}
export const flag = (name) => process.env[name] === "1" || process.env[name] === "true";

// Keys live OUTSIDE the repository by default. `examples/` is a tracked tree;
// a real private key must never land in it, gitignore or not.
export const DEFAULT_KEYSTORE = path.join(os.homedir(), ".strk20", "mainnet-keystore");
/** Expand a leading `~` so STRK20_KEYSTORE=~/... works from a shell or a .env file. */
export function expandHome(p) {
  if (p === "~") return os.homedir();
  if (p.startsWith("~/")) return path.join(os.homedir(), p.slice(2));
  return p;
}

export const CFG = () => ({
  keystore: expandHome(env("STRK20_KEYSTORE", DEFAULT_KEYSTORE)),
  rpc: env("STRK20_RPC", "https://starknet.publicnode.com"),
  pool: env("STRK20_POOL", "0x040337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a"),
  strk: env("STRK20_STRK", "0x04718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d"),
  accountClass: env("STRK20_ACCOUNT_CLASS", "0x05b4b537eaa2399e3aa99c4e2e0208ebd6c71bc1467938cd52c798c601e43564"),
  prover: env("STRK20_PROVER", "https://transaction-prover.alpha-mainnet.sw-dev.io"),
  discovery: env("STRK20_DISCOVERY", "https://discovery-service.alpha-mainnet.sw-dev.io"),
  feeMarginPercent: Number(env("STRK20_FEE_MARGIN_PERCENT", "0")),
  dryRun: flag("DRY_RUN"),
  force: flag("FORCE"),
});

// The sequencer rejects proofs whose base block is too recent. Observed cutoff
// on Sepolia: head-7. Upstream's own client uses head-9, which also absorbs the
// ~4 s proving round trip. Do not raise this without re-measuring.
export const PROVING_BLOCK_DEPTH = 9;

export const WAIT_OPTIONS = {
  successStates: [TransactionFinalityStatus.PRE_CONFIRMED, TransactionFinalityStatus.ACCEPTED_ON_L2, TransactionFinalityStatus.ACCEPTED_ON_L1],
  retryInterval: 3000,
};

// ── output ───────────────────────────────────────────────────────────────────
export const log = (...a) => console.log(new Date().toISOString().slice(11, 19), ...a);
export const step = (s) => console.log(`\n\x1b[1m── ${s}\x1b[0m`);
export function fail(msg, code = 1) {
  console.error(`\n\x1b[31mERROR\x1b[0m ${msg}\n`);
  process.exit(code);
}
export const STRK = 10n ** 18n;
export const fmt = (wei) => {
  const neg = wei < 0n;
  const v = neg ? -wei : wei;
  return (neg ? "-" : "") + `${v / STRK}.${(v % STRK).toString().padStart(18, "0").slice(0, 6)}`;
};
export function toWei(decimalStrk, label) {
  const s = String(decimalStrk).trim();
  if (!/^\d+(\.\d+)?$/.test(s)) fail(`${label}: expected a decimal STRK amount, got "${decimalStrk}"`);
  const [whole, frac = ""] = s.split(".");
  return BigInt(whole) * STRK + BigInt((frac + "0".repeat(18)).slice(0, 18));
}
/** Mask a secret for display. Never print the value itself. */
export const mask = (s) => `${String(s).slice(0, 6)}…${String(s).slice(-4)} (${String(s).length} chars)`;

// ── keystore ─────────────────────────────────────────────────────────────────
export function keystorePaths(dir) {
  return {
    dir,
    account: path.join(dir, "account.json"),
    viewingKey: path.join(dir, "viewing-key.hex"),
    state: path.join(dir, "state.json"),
  };
}
export function loadKeystore(cfg) {
  const p = keystorePaths(cfg.keystore);
  if (!fs.existsSync(p.account)) {
    fail(`no account at ${p.account}\n  run:  node 01-create-account.mjs`);
  }
  const a = JSON.parse(fs.readFileSync(p.account, "utf8"));
  if (a.chain_id !== constants.StarknetChainId.SN_MAIN) {
    fail(`keystore ${p.account} was created for chain ${a.chain_id}, not SN_MAIN`);
  }
  if (a.pool_address.toLowerCase() !== cfg.pool.toLowerCase() && BigInt(a.pool_address) !== BigInt(cfg.pool)) {
    fail(`keystore was created for pool ${a.pool_address} but STRK20_POOL is ${cfg.pool}.\n` + `  The viewing key is derived from the pool address — they must match.`);
  }
  return { ...a, paths: p };
}
export function readState(cfg) {
  const p = keystorePaths(cfg.keystore).state;
  return fs.existsSync(p) ? JSON.parse(fs.readFileSync(p, "utf8")) : {};
}
export function writeState(cfg, patch) {
  const p = keystorePaths(cfg.keystore).state;
  const s = { ...readState(cfg), ...patch };
  fs.writeFileSync(p, JSON.stringify(s, null, 2) + "\n", { mode: 0o600 });
  return s;
}
/** Refuse to redo a completed step unless FORCE=1. */
export function guardStep(cfg, name) {
  const done = readState(cfg)[name];
  if (done && !cfg.force) {
    log(`step "${name}" already completed: tx ${done.hash} in block ${done.block}`);
    log(`nothing to do. Re-run with FORCE=1 to do it again (this spends real STRK).`);
    process.exit(0);
  }
}

// ── viewing key ──────────────────────────────────────────────────────────────
// Canonical derivation, byte-compatible with the upstream wallet demo
// (demo/src/session.ts deriveViewingKey): sign starknetKeccak("<chainId>:<pool>")
// with the account key, Poseidon-fold (r,s), reduce mod the curve order and
// canonicalise into the lower half (the pool asserts 1 <= k < ORDER/2).
// Stark ECDSA in starknet.js is RFC-6979 deterministic, so this is stable.
export function deriveViewingKey(privateKey, chainId, poolAddress) {
  const messageHash = hash.starknetKeccak(`${chainId}:${poolAddress}`);
  const signature = ec.starkCurve.sign(`0x${messageHash.toString(16)}`, privateKey);
  const folded = BigInt(hash.computePoseidonHashOnElements([signature.r, signature.s]));
  const order = ec.starkCurve.CURVE.n;
  const reduced = folded % order;
  const canonical = reduced < MAX_VIEWING_KEY ? reduced : order - reduced;
  return canonical === 0n ? 1n : canonical;
}

// ── chain ────────────────────────────────────────────────────────────────────
export function providerFor(cfg) {
  return new RpcProvider({ nodeUrl: cfg.rpc, batch: false });
}
export function accountFor(cfg, ks, provider) {
  return new Account({ provider, address: ks.account_address, signer: ks.account_private_key, cairoVersion: "1" });
}

/** Hard stop unless the node really is mainnet. Every write script calls this. */
export async function guardChain(provider, cfg) {
  let chainId, spec;
  try {
    chainId = await provider.getChainId();
    spec = await provider.getSpecVersion();
  } catch (e) {
    fail(`cannot reach STRK20_RPC=${cfg.rpc}\n  ${String(e.message ?? e).slice(0, 300)}`);
  }
  if (chainId !== constants.StarknetChainId.SN_MAIN) {
    fail(`REFUSING: STRK20_RPC reports chain id ${chainId}, which is not SN_MAIN.\n` + `  These scripts are mainnet-only by construction.`);
  }
  const [maj, min] = spec.split(".").map(Number);
  if (maj === 0 && min < 10) {
    fail(`STRK20_RPC serves JSON-RPC spec ${spec}. v3 proof-carrying transactions need >= 0.10.\n` + `  Use a node on 0.10+ (verified: https://starknet.publicnode.com).`);
  }
  return { chainId, spec };
}

export const view = async (provider, contract, entrypoint, calldata = []) => provider.callContract({ contractAddress: contract, entrypoint, calldata }, "latest");
export const viewU256 = async (provider, contract, entrypoint, calldata = []) => {
  const [lo, hi] = await view(provider, contract, entrypoint, calldata);
  return BigInt(lo) + (BigInt(hi) << 128n);
};
export const balanceOf = (provider, cfg, addr) => viewU256(provider, cfg.strk, "balance_of", [addr]);
export const allowanceOf = (provider, cfg, owner) => viewU256(provider, cfg.strk, "allowance", [owner, cfg.pool]);
export const approveCall = (cfg, amount) => ({ contractAddress: cfg.strk, entrypoint: "approve", calldata: [cfg.pool, ...Object.values(cairo.uint256(amount)).map(String)] });

/** Live pool configuration. Nothing about the pool is hardcoded in the scripts. */
export async function readPool(provider, cfg) {
  const [[version], [feeAmount], [feeCollector], [screener], [validity], [paused]] = await Promise.all([
    view(provider, cfg.pool, "get_version"),
    view(provider, cfg.pool, "get_fee_amount"),
    view(provider, cfg.pool, "get_fee_collector"),
    view(provider, cfg.pool, "get_screener_public_key"),
    view(provider, cfg.pool, "get_proof_validity_blocks"),
    view(provider, cfg.pool, "is_paused"),
  ]);
  const classHash = await provider.getClassHashAt(cfg.pool, "latest");
  return {
    classHash,
    versionFelt: BigInt(version),
    version: Buffer.from(BigInt(version).toString(16), "hex").toString(),
    fee: BigInt(feeAmount),
    feeCollector,
    screener,
    screeningEnforced: BigInt(screener) !== 0n,
    proofValidityBlocks: Number(BigInt(validity)),
    paused: BigInt(paused) !== 0n,
  };
}

export async function printPool(provider, cfg) {
  const p = await readPool(provider, cfg);
  step("pool (read live, nothing hardcoded)");
  log(`address        ${cfg.pool}`);
  log(`class hash     ${p.classHash}`);
  log(`version        "${p.version}"`);
  log(`fee_amount     ${fmt(p.fee)} STRK   <- charged from your PUBLIC balance on every apply_actions`);
  log(`fee_collector  ${p.feeCollector}`);
  log(`screening      ${p.screeningEnforced ? "ENFORCED" : "off"}`);
  log(`proof validity ${p.proofValidityBlocks} blocks`);
  if (p.paused) fail("the pool is PAUSED — no action can succeed right now.");
  return p;
}

// ── fees ─────────────────────────────────────────────────────────────────────
export const boundsCeiling = (rb) => ["l1_gas", "l2_gas", "l1_data_gas"].reduce((acc, k) => acc + (rb?.[k] ? BigInt(rb[k].max_amount) * BigInt(rb[k].max_price_per_unit) : 0n), 0n);

/** Apply STRK20_FEE_MARGIN_PERCENT to an estimate's resource bounds. */
export function withMargin(rb, percent) {
  if (!percent) return rb;
  const bump = (v) => (BigInt(v) * BigInt(100 + percent)) / 100n;
  const out = {};
  for (const k of ["l1_gas", "l2_gas", "l1_data_gas"]) {
    if (rb?.[k]) out[k] = { max_amount: num.toHex(bump(rb[k].max_amount)), max_price_per_unit: rb[k].max_price_per_unit };
  }
  return out;
}

/**
 * Estimate, print a full breakdown, and refuse to continue if the balance
 * cannot clear the resource-bounds ceiling. Returns { bounds, ceiling }.
 *
 * `spentDuringExecution` is value the transaction moves out of the account on
 * top of gas (deposit amount + pool fee), so the caller is told the true total.
 */
export async function estimateAndCheck(account, provider, cfg, calls, details, { spentDuringExecution = 0n, label = "transaction" } = {}) {
  let est;
  try {
    est = await account.estimateInvokeFee(calls, { tip: 0n, ...details });
  } catch (e) {
    const inner = explainRpcError(e);
    fail(`fee estimation failed for the ${label}; nothing was submitted.\n  ${inner}\n\n` + hintFor(inner));
  }
  const bounds = withMargin(est.resourceBounds, cfg.feeMarginPercent);
  const ceiling = boundsCeiling(bounds);
  const balance = await balanceOf(provider, cfg, account.address);

  step("fee breakdown");
  log(`node estimate (overall_fee)   ${fmt(BigInt(est.overall_fee))} STRK`);
  log(`resource-bounds ceiling       ${fmt(ceiling)} STRK   <- your balance must cover THIS at validation`);
  if (cfg.feeMarginPercent) log(`  (includes STRK20_FEE_MARGIN_PERCENT=${cfg.feeMarginPercent})`);
  if (spentDuringExecution > 0n) log(`moved out during execution    ${fmt(spentDuringExecution)} STRK   (deposit + pool fee)`);
  log(`your balance                  ${fmt(balance)} STRK`);
  const need = ceiling > spentDuringExecution ? ceiling : spentDuringExecution;
  if (balance < need) {
    fail(
      `INSUFFICIENT BALANCE — nothing was submitted, nothing was spent.\n` +
        `  account ${account.address}\n` +
        `  has     ${fmt(balance)} STRK\n` +
        `  needs   ${fmt(need)} STRK  (short by ${fmt(need - balance)} STRK)\n\n` +
        `  Send at least ${fmt(need - balance)} more STRK to that address and re-run.\n` +
        `  Note the ceiling is a cap, not the bill: recent mainnet pool transactions\n` +
        `  actually paid ~3 STRK of gas against a ~7-17 STRK ceiling.`
    );
  }
  log(`headroom                      ${fmt(balance - need)} STRK  \x1b[32mOK\x1b[0m`);
  return { bounds, ceiling, estimate: BigInt(est.overall_fee) };
}

/**
 * Starknet RPC errors arrive as a wall of JSON with the useful sentence buried
 * several levels down. Dig out the most specific human-readable part.
 */
export function explainRpcError(e) {
  const raw = String(e?.message ?? e ?? "");

  // starknet.js RpcError carries `baseError`; the node nests the real cause as
  //   { code, message, data: { execution_error: { contract_address, error: {
  //       contract_address, error: "0x… ('ERC20: insufficient balance')" } } } }
  // recursing through `error` until the leaf is a string. Walk to that leaf.
  let node = e?.baseError ?? e?.cause ?? null;
  if (!node) {
    // fall back to the JSON embedded in the message text
    const brace = raw.indexOf('{"code"');
    if (brace >= 0) { try { node = JSON.parse(raw.slice(brace)); } catch { /* ignore */ } }
  }
  let contract = null;
  let leaf = null;
  let guard = 0;
  let cur = node;
  while (cur && typeof cur === "object" && guard++ < 24) {
    if (cur.contract_address) contract = cur.contract_address;
    const next = cur.execution_error ?? cur.revert_error ?? cur.error ?? cur.data;
    if (typeof next === "string") { leaf = next; break; }
    if (next === undefined) { if (typeof cur.message === "string") leaf = cur.message; break; }
    cur = next;
  }

  if (!leaf) {
    // last resort: pull the deepest quoted Cairo short string out of the raw text
    const all = [...raw.matchAll(/\('([^']{2,200})'\)/g)].map((m) => m[1]);
    leaf = all.length ? all[all.length - 1] : null;
  }
  if (!leaf) {
    const m = raw.match(/"message"\s*:\s*"((?:[^"\\]|\\.){2,300})"/);
    leaf = m ? m[1] : raw.slice(0, 300);
  }

  // A Cairo panic reads `0x4552… ('ERC20: insufficient balance')` — keep the words.
  const decoded = leaf.match(/\('([^']*)'\)/);
  let out = (decoded ? decoded[1] : leaf).replace(/\\n/g, " ").replace(/\s+/g, " ").trim();
  const top = node?.message && node.message !== out ? `${node.message}: ` : "";
  if (contract && !out.includes(contract)) out += `  [reverted in ${contract.slice(0, 12)}…]`;
  return (top + out).slice(0, 400);
}

function hintFor(msg) {
  if (/too recent/i.test(msg)) return `  Hint: the proving block was too close to the head. Re-run — the script proves at head-${PROVING_BLOCK_DEPTH}.`;
  if (/allowance/i.test(msg)) return `  Hint: the ERC-20 allowance to the pool is too small. The pool pulls BOTH the deposit\n  and the ${"fee"} via transferFrom, so the approve must cover deposit + fee.`;
  if (/screening|10000/i.test(msg)) return `  Hint: the deposit screening check rejected this address. Screening is fail-closed;\n  there is no override. Try a different funding source.`;
  if (/balance/i.test(msg)) return `  Hint: fund the account with more STRK.`;
  if (/nonce/i.test(msg)) return `  Hint: a previous transaction may still be in flight. Wait for it, then re-run.`;
  return `  Nothing was submitted. Fix the cause and re-run; the scripts are safe to repeat.`;
}

// ── submission ───────────────────────────────────────────────────────────────
const EVENT_NAMES = ["ViewingKeySet", "Withdrawal", "Deposit", "OpenNoteCreated", "OpenNoteDeposited", "EncNoteCreated", "NoteUsed", "ExternalContractInvoked"];
const EVENT_BY_SELECTOR = new Map(EVENT_NAMES.map((n) => [BigInt(hash.getSelectorFromName(n)), n]));

export function reportPoolEvents(receipt, cfg) {
  step("pool events");
  const rows = (receipt.events ?? []).filter((e) => BigInt(e.from_address) === BigInt(cfg.pool));
  if (rows.length === 0) log("(none — that is a problem; the pool emitted nothing)");
  for (const e of rows) {
    const name = EVENT_BY_SELECTOR.get(BigInt(e.keys[0])) ?? `unknown(${e.keys[0]})`;
    log(`  ${name.padEnd(16)} keys=${JSON.stringify(e.keys.slice(1))} data=${JSON.stringify(e.data)}`);
  }
  return rows.map((e) => ({ name: EVENT_BY_SELECTOR.get(BigInt(e.keys[0])) ?? e.keys[0], keys: e.keys, data: e.data }));
}

export async function submit(account, provider, cfg, calls, details, bounds) {
  step("submitting");
  const tx = await account.execute(calls, { tip: 0n, ...details, ...(bounds ? { resourceBounds: bounds } : {}) });
  log(`tx hash  ${tx.transaction_hash}`);
  log(`explorer https://voyager.online/tx/${tx.transaction_hash}`);
  const rc = await provider.waitForTransaction(tx.transaction_hash, WAIT_OPTIONS);
  log(`status   ${rc.execution_status} in block ${rc.block_number}`);
  log(`gas paid ${fmt(BigInt(rc.actual_fee?.amount ?? 0))} STRK`);
  if (!rc.isSuccess?.()) {
    console.error(JSON.stringify(rc, null, 1).slice(0, 2000));
    fail(`the transaction REVERTED on chain. Gas was still charged. See the dump above.`);
  }
  return { tx, receipt: rc };
}

/** The block to prove against: old enough for the sequencer to accept the proof. */
export async function provingBlock(provider) {
  const head = await provider.getBlockNumber();
  return { head, provingBlockId: head - PROVING_BLOCK_DEPTH };
}

export function dryRunStop(cfg, what) {
  if (!cfg.dryRun) return false;
  step("DRY RUN");
  log(`DRY_RUN=1 — everything up to submission succeeded (${what}).`);
  log(`Nothing was sent, nothing was spent.`);
  log(`Set DRY_RUN=0 in your environment to submit for real.`);
  return true;
}

// ── SDK wiring ───────────────────────────────────────────────────────────────
// Shared by 03/04/05. Plain HTTP (no OHTTP): these scripts run headless on your
// own machine, so the relay adds nothing but a dependency.
export async function makeTransfers(cfg, ks, account, chainId = constants.StarknetChainId.SN_MAIN) {
  const { createPrivateTransfers, ProvingServiceProofProvider, IndexerDiscoveryProvider } = await import("@starkware-libs/starknet-privacy-sdk");
  return createPrivateTransfers({
    account,
    viewingKeyProvider: { getViewingKey: async () => BigInt(ks.viewing_key) },
    provingProvider: new ProvingServiceProofProvider(cfg.prover, chainId, { requestTimeoutMs: 180_000 }),
    discoveryProvider: new IndexerDiscoveryProvider(cfg.discovery, cfg.pool),
    poolContractAddress: cfg.pool,
  });
}

/** Unspent notes for the STRK token, newest last. Fails loudly if there are none. */
export async function spendableNotes(transfers, cfg, { required = true } = {}) {
  step("discovering your notes");
  let found;
  try {
    found = await transfers.discoverNotes();
  } catch (e) {
    fail(`note discovery failed against ${cfg.discovery}\n  ${String(e.message ?? e).slice(0, 300)}\n` + `  The discovery service may be down. Nothing was spent.`);
  }
  const notes = found.notes.get(BigInt(cfg.strk)) ?? [];
  for (const n of notes) log(`  note ${n.id}  ${fmt(n.amount)} STRK  created ${n.created}  open ${n.open ?? false}`);
  if (notes.length === 0) {
    if (!required) return notes;
    fail(`you have no unspent STRK notes in the pool.\n  Run 03-shield.mjs first, and give it a minute — a note is only spendable\n  once the block it was created in is a few blocks behind the head.`);
  }
  log(`total shielded: ${fmt(notes.reduce((a, n) => a + n.amount, 0n))} STRK across ${notes.length} note(s)`);
  return notes;
}

/**
 * Prove, then hand back the call + proof details. Common to all three pool
 * scripts: prove against head-PROVING_BLOCK_DEPTH, report what came back.
 */
export async function buildAndProve(transfers, chain, provider) {
  const { head, provingBlockId } = await provingBlock(provider);
  step("proving");
  log(`head ${head}; proving against block ${provingBlockId} (head-${PROVING_BLOCK_DEPTH})`);
  let invocation;
  try {
    invocation = await chain.createProofInvocation({ provingBlockId });
  } catch (e) {
    fail(`could not build the action set: ${explainRpcError(e)}`);
  }
  if (invocation.warnings?.length) log(`warnings: ${JSON.stringify(invocation.warnings)}`);
  const t0 = Date.now();
  let result;
  try {
    result = await transfers.executeWithInvocation(invocation, provingBlockId);
  } catch (e) {
    fail(`the proving service rejected the request or timed out.\n  ${explainRpcError(e)}\n` + `  Nothing was submitted. Check the prover URL and retry.`);
  }
  const { callAndProof } = result;
  log(`proved in ${((Date.now() - t0) / 1000).toFixed(1)}s`);
  log(`proof ${Math.round((callAndProof.proof.data?.length ?? 0) * 0.75)} bytes, ${callAndProof.proof.proofFacts.length} proof facts`);
  log(`screening attestation: ${callAndProof.proof.additionalData?.signature ? "present (required for deposits)" : "absent (not required for this action)"}`);
  const details = callAndProof.proof.proofFacts?.length ? { proofFacts: callAndProof.proof.proofFacts, proof: callAndProof.proof.data } : {};
  return { callAndProof, details, provingBlockId };
}

/**
 * Build the call list for a pool transaction, batching the ERC-20 approve into
 * the SAME transaction when the allowance is short.
 *
 * `collect_fee` pulls the pool fee from the CALLER via transferFrom on every
 * apply_actions, deposit or not — so the allowance must cover
 * (deposit amount, if any) + fee. Batching is safe: the proof facts bind the
 * pool's own action span, not the transaction's call list.
 */
export async function callsWithApprove(provider, cfg, owner, poolCall, needAllowance) {
  const have = await allowanceOf(provider, cfg, owner);
  log(`allowance to pool: ${fmt(have)} STRK; this transaction needs ${fmt(needAllowance)} STRK`);
  if (have >= needAllowance) return [poolCall];
  log(`batching approve(${fmt(needAllowance)} STRK) into the same transaction`);
  return [approveCall(cfg, needAllowance), poolCall];
}
