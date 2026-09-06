import { CallData, ec, hash, stark } from "starknet";
import { ACCOUNT_CLASS, NETWORKS, type Network } from "./network.ts";

export type Action = "deploy" | "shield" | "transfer" | "withdraw";
export interface Wallet {
  version: 1;
  network: Network;
  address: string;
  publicKey: string;
  signingKey: string;
  viewingKey: string;
  classHash: string;
  pool: string;
  completed: Partial<Record<Action, { hash: string; block: number }>>;
  pending?: { action: Action; hash?: string };
}

// This database is deliberately separate from the disposable discovery cache.
// The demo trusts same-origin storage. Export a backup before funding it.
function database(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open("strk20-demo-wallet", 1);
    request.onupgradeneeded = () => request.result.createObjectStore("wallet");
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

export async function loadWallet(
  network: Network,
): Promise<Wallet | undefined> {
  const db = await database();
  try {
    const result = await new Promise<unknown>((resolve, reject) => {
      const request = db
        .transaction("wallet")
        .objectStore("wallet")
        .get(network);
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error);
    });
    return result === undefined ? undefined : validateWallet(result);
  } finally {
    db.close();
  }
}

export async function saveWallet(wallet: Wallet): Promise<void> {
  const db = await database();
  try {
    await new Promise<void>((resolve, reject) => {
      const tx = db.transaction("wallet", "readwrite", {
        durability: "strict",
      });
      tx.objectStore("wallet").put(wallet, wallet.network);
      tx.oncomplete = () => resolve();
      tx.onerror = tx.onabort = () =>
        reject(tx.error ?? new Error("Wallet storage failed."));
    });
  } finally {
    db.close();
  }
}

function viewingKey(signingKey: string, network: Network): string {
  const config = NETWORKS[network];
  const message = hash.starknetKeccak(`${config.chainId}:${config.pool}`);
  const signature = ec.starkCurve.sign(`0x${message.toString(16)}`, signingKey);
  const order = ec.starkCurve.CURVE.n;
  const reduced =
    BigInt(hash.computePoseidonHashOnElements([signature.r, signature.s])) %
    order;
  const canonical = reduced < order / 2n ? reduced : order - reduced;
  return `0x${(canonical || 1n).toString(16)}`;
}

export function newWallet(network: Network): Wallet {
  const signingKey = stark.randomAddress();
  const publicKey = ec.starkCurve.getStarkKey(signingKey);
  return {
    version: 1,
    network,
    signingKey,
    publicKey,
    classHash: ACCOUNT_CLASS,
    pool: NETWORKS[network].pool,
    address: hash.calculateContractAddressFromHash(
      publicKey,
      ACCOUNT_CLASS,
      CallData.compile({ publicKey }),
      0,
    ),
    viewingKey: viewingKey(signingKey, network),
    completed: {},
  };
}

export function validateWallet(value: unknown): Wallet {
  if (!value || typeof value !== "object")
    throw new Error("Invalid wallet backup.");
  const w = value as Wallet;
  if (
    w.version !== 1 ||
    !Object.hasOwn(NETWORKS, w.network) ||
    typeof w.signingKey !== "string" ||
    !w.completed ||
    typeof w.completed !== "object"
  )
    throw new Error("Unsupported wallet backup.");
  const config = NETWORKS[w.network];
  const publicKey = ec.starkCurve.getStarkKey(w.signingKey);
  const address = hash.calculateContractAddressFromHash(
    publicKey,
    ACCOUNT_CLASS,
    CallData.compile({ publicKey }),
    0,
  );
  if (
    BigInt(w.classHash) !== BigInt(ACCOUNT_CLASS) ||
    BigInt(w.pool) !== BigInt(config.pool) ||
    BigInt(w.publicKey) !== BigInt(publicKey) ||
    BigInt(w.address) !== BigInt(address) ||
    BigInt(w.viewingKey) !== BigInt(viewingKey(w.signingKey, w.network))
  ) {
    throw new Error("The backup keys, address, pool or network do not agree.");
  }
  const actions = new Set(["deploy", "shield", "transfer", "withdraw"]);
  const txHash = (value: unknown) =>
    typeof value === "string" && /^0x[0-9a-fA-F]{1,64}$/.test(value);
  if (
    Array.isArray(w.completed) ||
    Object.entries(w.completed).some(
      ([action, record]) =>
        !actions.has(action) ||
        !record ||
        !txHash(record.hash) ||
        !Number.isSafeInteger(record.block) ||
        record.block < 0,
    ) ||
    (w.pending &&
      (!actions.has(w.pending.action) ||
        (w.pending.hash !== undefined && !txHash(w.pending.hash))))
  )
    throw new Error("Invalid transaction history in wallet backup.");
  return w;
}

export function exportWallet(wallet: Wallet): void {
  const url = URL.createObjectURL(
    new Blob([JSON.stringify(wallet, null, 2)], { type: "application/json" }),
  );
  const link = document.createElement("a");
  link.href = url;
  link.download = `strk20-${wallet.network}-wallet-backup.json`;
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
