// Derive the pool viewing key from the operator's account key and persist it,
// mode 0600, at STRK20_KEY_FILE. Run this BEFORE any transaction: if a shield
// lands and the key is lost, the note is unrecoverable.
//
// The key is derived, not random — losing the file is survivable as long as the
// account private key survives — but the file also carries the transaction
// record the verify scripts append to.
import { RpcProvider, ec } from "starknet";
import {
  RPC, POOL, STRK, SEPOLIA_CHAIN_ID, ACCOUNT_NAME,
  loadAccount, deriveViewingKey, keyFilePath, writeKeyFile,
} from "./lib.mjs";
import fs from "node:fs";

const name = process.argv[2] ?? ACCOUNT_NAME;
const a = loadAccount(name);
const vk = deriveViewingKey(a.privateKey, SEPOLIA_CHAIN_ID, POOL);
const provider = new RpcProvider({ nodeUrl: RPC });

const out = {
  _comment:
    "Sepolia STRK20 privacy-pool key material. TESTNET ONLY — never point this at a mainnet account. " +
    "viewing_key is derived deterministically from the account private key via " +
    "starknetKeccak('<chainId>:<pool>') -> stark ECDSA sign -> poseidon(r,s) mod curve order, " +
    "canonicalised into the lower half (SDK MAX_VIEWING_KEY). Same derivation as the upstream " +
    "demo (demo/src/session.ts), so any wallet holding the account key recomputes it.",
  network: "sepolia",
  chain_id: SEPOLIA_CHAIN_ID,
  rpc: RPC,
  pool_address: POOL,
  strk_token: STRK,
  account_name: name,
  account_address: a.address,
  account_public_key: a.publicKey,
  viewing_key: "0x" + vk.toString(16),
  viewing_key_decimal: vk.toString(10),
  derivation: {
    message: `${SEPOLIA_CHAIN_ID}:${POOL}`,
    scheme: "starknetKeccak(message) -> ec.starkCurve.sign(hash, privKey) -> poseidon([r,s]) mod n, canonical lower half",
    sdk_version: "0.14.3-rc.5 (built from starkware-libs/starknet-privacy tag PRIVACY-0.14.3-RC.5)",
  },
  created_at: new Date().toISOString(),
  transactions: {},
};

// Preserve an existing transaction record when re-running.
const file = keyFilePath(name);
if (fs.existsSync(file)) {
  const prev = JSON.parse(fs.readFileSync(file, "utf8"));
  out.transactions = prev.transactions ?? {};
  out.created_at = prev.created_at ?? out.created_at;
}
writeKeyFile(out, name);

console.log("wrote", file, "(mode 0600)");
console.log("account", a.address);
console.log("viewing key (masked):", out.viewing_key.slice(0, 8) + "…" + out.viewing_key.slice(-4), `len ${out.viewing_key.length}`);
const [pk] = await provider.callContract({ contractAddress: POOL, entrypoint: "get_public_key", calldata: [a.address] }, "latest");
console.log("on-chain pool public key for this account:", BigInt(pk) === 0n ? "UNREGISTERED" : pk);
console.log("expected pool public key from viewing key:", ec.starkCurve.getStarkKey("0x" + vk.toString(16)));
