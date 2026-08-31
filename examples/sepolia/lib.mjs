// Shared configuration + helpers for the Sepolia demo runs.
//
// SECRETS: this file never contains key material and never writes any into the
// repository. Every secret is read from a path or an environment variable the
// operator supplies (see .env.example):
//
//   STRK20_ACCOUNT_PRIVATE_KEY + STRK20_ACCOUNT_ADDRESS   — direct, or
//   STRK20_ACCOUNTS_FILE (+ STRK20_ACCOUNT / STRK20_ACCOUNTS_NETWORK)
//                                                          — an sncast accounts file
//   STRK20_KEY_FILE                                        — where the derived
//                                                            viewing key is persisted (0600)
//
// The viewing key is DERIVED from the account private key, never typed in and
// never printed in full.
import fs from "node:fs";
import path from "node:path";
import { hash, ec } from "starknet";
import { MAX_VIEWING_KEY } from "@starkware-libs/starknet-privacy-sdk";

// ── public network parameters (not secrets; override via env if the pool moves)
export const RPC = process.env.STRK20_RPC ?? "https://starknet-sepolia-rpc.publicnode.com";
export const PROVER_URL = process.env.STRK20_PROVER_URL ?? "https://transaction-prover.alpha-sepolia.sw-dev.io";
export const DISCOVERY_URL = process.env.STRK20_DISCOVERY_URL ?? "https://discovery-service.alpha-sepolia.sw-dev.io";
export const POOL = process.env.STRK20_POOL ?? "0x0254a6b2997ef52e9f830ce1f543f6b29768295e8d17e2267d672c552cfe0d91";
export const STRK = process.env.STRK20_STRK_TOKEN ?? "0x04718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d";
export const SEPOLIA_CHAIN_ID = "0x534e5f5345504f4c4941";

// The sequencer rejects proofs whose base block is too recent; observed cutoff
// head-7. Upstream uses head-9 (demo/src/hooks/useTransactions.ts
// PROVING_BLOCK_DEPTH), which also covers the ~4 s proving round trip.
export const PROVING_BLOCK_DEPTH = Number(process.env.STRK20_PROVING_BLOCK_DEPTH ?? 9);

export const ACCOUNT_NAME = process.env.STRK20_ACCOUNT ?? "strk20test";
export const DRY_RUN = process.env.DRY_RUN === "1";

// Working artifacts (call-and-proof dumps, logs). Never key material.
export const OUT_DIR = process.env.STRK20_OUT_DIR ?? path.join(process.cwd(), ".local");

function required(name, why) {
  const v = process.env[name];
  if (!v) throw new Error(`${name} is not set — ${why}. Copy .env.example, fill it in, and 'set -a; . ./.env; set +a'.`);
  return v;
}

/**
 * Load the operator's account. Two supported shapes, both operator-supplied:
 *   1. STRK20_ACCOUNT_PRIVATE_KEY + STRK20_ACCOUNT_ADDRESS in the environment
 *   2. STRK20_ACCOUNTS_FILE pointing at an sncast accounts JSON
 */
export function loadAccount(name = ACCOUNT_NAME) {
  if (process.env.STRK20_ACCOUNT_PRIVATE_KEY) {
    return {
      name,
      address: required("STRK20_ACCOUNT_ADDRESS", "needed alongside STRK20_ACCOUNT_PRIVATE_KEY"),
      privateKey: process.env.STRK20_ACCOUNT_PRIVATE_KEY,
      publicKey: process.env.STRK20_ACCOUNT_PUBLIC_KEY ?? null,
      deployed: true,
    };
  }
  const file = required("STRK20_ACCOUNTS_FILE", "point it at your sncast accounts file, or set STRK20_ACCOUNT_PRIVATE_KEY");
  const network = process.env.STRK20_ACCOUNTS_NETWORK ?? "alpha-sepolia";
  const all = JSON.parse(fs.readFileSync(file, "utf8"));
  const a = all[network]?.[name];
  if (!a) throw new Error(`account ${network}/${name} not found in ${file}`);
  return { name, address: a.address, privateKey: a.private_key, publicKey: a.public_key, deployed: a.deployed };
}

export function listAccounts() {
  if (process.env.STRK20_ACCOUNT_PRIVATE_KEY) return [ACCOUNT_NAME];
  const file = required("STRK20_ACCOUNTS_FILE", "needed to list accounts");
  const network = process.env.STRK20_ACCOUNTS_NETWORK ?? "alpha-sepolia";
  return Object.keys(JSON.parse(fs.readFileSync(file, "utf8"))[network] ?? {});
}

/** Path of the operator's key file. Must be outside the repository. */
export function keyFilePath(name = ACCOUNT_NAME) {
  const p = required("STRK20_KEY_FILE", "the derived viewing key must be written somewhere you control, outside this repo");
  return p.includes("{account}") ? p.replaceAll("{account}", name) : p;
}

export function readKeyFile(name = ACCOUNT_NAME) {
  return JSON.parse(fs.readFileSync(keyFilePath(name), "utf8"));
}

/** Write the key file with 0600, creating parent directories as needed. */
export function writeKeyFile(obj, name = ACCOUNT_NAME) {
  const file = keyFilePath(name);
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, JSON.stringify(obj, null, 2) + "\n", { mode: 0o600 });
  fs.chmodSync(file, 0o600);
  return file;
}

/** Merge a patch into key-file `transactions` without touching anything else. */
export function recordTransaction(patch, name = ACCOUNT_NAME) {
  const j = readKeyFile(name);
  j.transactions = { ...j.transactions, ...patch };
  writeKeyFile(j, name);
}

export function outFile(basename) {
  fs.mkdirSync(OUT_DIR, { recursive: true });
  return path.join(OUT_DIR, basename);
}

export const fmt = (wei) => (Number(wei) / 1e18).toFixed(6);
export const mask = (s) => `${String(s).slice(0, 6)}…${String(s).slice(-4)} (len ${String(s).length})`;
export const log = (...a) => console.log(new Date().toISOString().slice(11, 19), ...a);

/**
 * Canonical viewing-key derivation, identical to upstream demo/src/session.ts:
 *   starknetKeccak("<chainId>:<pool>") -> Stark ECDSA sign (RFC-6979, deterministic)
 *   -> poseidon([r,s]) mod curve order -> canonicalised into the lower half.
 * Deterministic: anyone holding the account private key recomputes the same key.
 */
export function deriveViewingKey(privateKey, chainId = SEPOLIA_CHAIN_ID, poolAddress = POOL) {
  const messageHash = hash.starknetKeccak(`${chainId}:${poolAddress}`);
  const signature = ec.starkCurve.sign(`0x${messageHash.toString(16)}`, privateKey);
  const folded = BigInt(hash.computePoseidonHashOnElements([signature.r, signature.s]));
  const order = ec.starkCurve.CURVE.n;
  const reduced = folded % order;
  const canonical = reduced < MAX_VIEWING_KEY ? reduced : order - reduced;
  return canonical === 0n ? 1n : canonical;
}

/** Plain JSON-RPC against the configured node (used where starknet.js hides the raw shape). */
export async function rpc(method, params) {
  const r = await fetch(RPC, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }),
  });
  const j = await r.json();
  if (j.error) throw new Error(method + ": " + JSON.stringify(j.error));
  return j.result;
}

/** Sum of the resource-bounds ceiling — this, not overall_fee, is what the balance must clear. */
export const boundsCeiling = (rb) =>
  ["l1_gas", "l2_gas", "l1_data_gas"].reduce(
    (acc, k) => acc + (rb?.[k] ? BigInt(rb[k].max_amount) * BigInt(rb[k].max_price_per_unit) : 0n),
    0n,
  );
