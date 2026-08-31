// Read-only pre-flight: chain, pool parameters, account balances, prover and
// discovery health. Run this first; it makes no transaction and costs nothing.
import { RpcProvider, constants } from "starknet";
import {
  RPC, POOL, STRK, PROVER_URL, DISCOVERY_URL, SEPOLIA_CHAIN_ID,
  loadAccount, listAccounts, deriveViewingKey, fmt, mask,
} from "./lib.mjs";

const provider = new RpcProvider({ nodeUrl: RPC });
const call = (contractAddress, entrypoint, calldata = []) => provider.callContract({ contractAddress, entrypoint, calldata }, "latest");

const chainId = await provider.getChainId();
console.log("chain id      :", chainId, chainId === constants.StarknetChainId.SN_SEPOLIA ? "(SN_SEPOLIA ok)" : "(!! NOT SEPOLIA — the run scripts will refuse)");
console.log("spec version  :", await provider.getSpecVersion());
console.log("head block    :", await provider.getBlockNumber());

const [version] = await call(POOL, "get_version");
const [fee] = await call(POOL, "get_fee_amount");
const [collector] = await call(POOL, "get_fee_collector");
const [screener] = await call(POOL, "get_screener_public_key");
const [validity] = await call(POOL, "get_proof_validity_blocks");
const [paused] = await call(POOL, "is_paused");
console.log("\npool", POOL);
console.log("  class_hash           :", await provider.getClassHashAt(POOL, "latest"));
console.log("  version              :", BigInt(version).toString(), `("${Buffer.from(BigInt(version).toString(16), "hex").toString()}")`);
console.log("  fee_amount           :", fee, `= ${fmt(BigInt(fee))} STRK`);
console.log("  fee_collector        :", collector);
console.log("  screener_public_key  :", screener, BigInt(screener) === 0n ? "(screening OFF)" : "(screening ENFORCED)");
console.log("  proof_validity_blocks:", BigInt(validity).toString());
console.log("  is_paused            :", BigInt(paused) === 0n ? "no" : "YES");

const names = listAccounts();
console.log("\naccounts:", names.join(", "));
let total = 0n;
for (const name of names) {
  const a = loadAccount(name);
  const [lo, hi] = await call(STRK, "balance_of", [a.address]);
  const bal = BigInt(lo) + (BigInt(hi) << 128n);
  total += bal;
  let nonce;
  try { nonce = await provider.getNonceForAddress(a.address, "latest"); } catch { nonce = "(not deployed)"; }
  const [pk] = await call(POOL, "get_public_key", [a.address]);
  console.log(`  ${name.padEnd(12)} ${a.address}  bal=${fmt(bal).padStart(12)} STRK  nonce=${nonce}  pool_pubkey=${BigInt(pk) === 0n ? "unregistered" : pk}`);
  // masked only — the viewing key is a secret and is never printed in full
  console.log(`               derived viewing key: ${mask("0x" + deriveViewingKey(a.privateKey, SEPOLIA_CHAIN_ID, POOL).toString(16))}`);
}
console.log("  TOTAL:", fmt(total), "STRK");

const r = await fetch(PROVER_URL, {
  method: "POST",
  headers: { "content-type": "application/json" },
  body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "starknet_specVersion", params: [] }),
});
console.log("\nprover   ", PROVER_URL, "->", r.status, JSON.stringify(await r.json()));
try {
  const h = await fetch(DISCOVERY_URL + "/health");
  console.log("discovery", DISCOVERY_URL + "/health", "->", h.status, (await h.text()).slice(0, 200));
} catch (e) {
  console.log("discovery health failed:", e.message);
}
