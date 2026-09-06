import {
  LocalDiscoveryProvider,
  type AccountDiscovery,
  type EngineInfo,
} from "strk20-discovery";
import {
  loadWallet,
  saveWallet,
  newWallet,
  validateWallet,
  exportWallet,
  type Wallet,
  type Action,
} from "./wallet.ts";
import {
  NETWORKS,
  STRK,
  formatStrk,
  parseStrk,
  type Network,
} from "./network.ts";
import { Operations } from "./operations.ts";
import { Transactions } from "./transactions.ts";
import { observeAt, type Comparison } from "./benchmark.ts";
import { element, mount, renderOperations, downloadJson } from "./ui.ts";

type Notes = Awaited<ReturnType<AccountDiscovery["discoverNotes"]>>;
let network: Network =
  localStorage.getItem("strk20-demo-network") === "mainnet"
    ? "mainnet"
    : "sepolia";
let wallet: Wallet | undefined,
  provider: LocalDiscoveryProvider | undefined,
  account: AccountDiscovery | undefined,
  transactions: Transactions | undefined;
let info: EngineInfo | undefined,
  notes: Notes | undefined,
  balance = 0n,
  deployed = false,
  busy = false,
  error = "";
let observedTransaction: string | undefined;
const comparisons: Comparison[] = [];
const operations = new Operations(() => {
  renderOperations(operations.entries);
  render();
});
mount(network);
const text = (id: string, value: string) => {
  element(id).textContent = value;
};
function latestPrivateTransaction() {
  return Object.entries(wallet?.completed ?? {})
    .filter(([action]) => action !== "deploy")
    .map(([, record]) => record)
    .sort((a, b) => b.block - a.block)[0];
}
function step(): {
  label: string;
  description: string;
  action:
    | Action
    | "create"
    | "fund"
    | "discover"
    | "resume"
    | "refresh"
    | "initialize";
} {
  if (!wallet)
    return {
      label: "Create demo wallet",
      description:
        "Create a temporary software wallet, then fund its public address with a small amount of STRK.",
      action: "create",
    };
  if (!transactions)
    return {
      label: "Initialize wallet",
      description: "Restore the wallet connection before continuing.",
      action: "initialize",
    };
  if (wallet.pending)
    return {
      label: "Resume pending transaction",
      description: "Check the existing transaction before sending another one.",
      action: "resume",
    };
  if (!deployed)
    return balance > 0n
      ? {
          label: "Deploy wallet",
          description:
            "Use the funded STRK to deploy this account on Starknet.",
          action: "deploy",
        }
      : {
          label: "Check funding",
          description:
            "Export a backup, then send a small amount of STRK to the address below. Keep enough public STRK for transaction fees.",
          action: "fund",
        };
  const latest = latestPrivateTransaction();
  if (latest && observedTransaction !== latest.hash)
    return {
      label: "Discover transaction",
      description:
        "Verify pool state and find this transaction’s effect locally.",
      action: "discover",
    };
  if (!wallet.completed.shield)
    return {
      label: "Shield STRK",
      description:
        "Move STRK into the privacy pool. The next spend uses notes found by our local discovery provider.",
      action: "shield",
    };
  if (!wallet.completed.transfer)
    return {
      label: "Transfer privately",
      description:
        "Send to yourself for a complete demo, or enter another registered private recipient.",
      action: "transfer",
    };
  if (!wallet.completed.withdraw)
    return {
      label: "Withdraw STRK",
      description:
        "Return private funds to a public wallet. Gas and the pool fee are paid from this demo wallet’s public balance.",
      action: "withdraw",
    };
  return {
    label: "Refresh private balance",
    description:
      "The flow is complete. Your backup also preserves access to any private change and remaining public STRK.",
    action: "refresh",
  };
}
function render(): void {
  const current = step();
  text("title", current.label);
  text("description", current.description);
  text("button-text", current.label);
  text("step", wallet ? "REAL TRANSACTIONS" : "GET STARTED");
  text(
    "connection",
    info?.verificationFailed
      ? "Verification failed"
      : info?.verifiedAt !== undefined && info.verifiedAt !== null
        ? `Block ${info.verifiedAt.toLocaleString()}`
        : "Not synced",
  );
  const button = element<HTMLButtonElement>("next");
  button.disabled = busy;
  button.setAttribute("aria-busy", String(busy));
  element<HTMLSelectElement>("network").disabled = busy;
  for (const id of ["import", "backup"])
    element<HTMLButtonElement>(id).disabled = busy;
  element("wallet").hidden = !wallet;
  element("amount-row").hidden = !["shield", "transfer", "withdraw"].includes(
    current.action,
  );
  element("recipient-row").hidden = !["transfer", "withdraw"].includes(
    current.action,
  );
  text(
    "recipient-label",
    current.action === "withdraw"
      ? "Public withdrawal address"
      : "Private recipient · blank sends to yourself",
  );
  if (wallet) {
    const link = element<HTMLAnchorElement>("address");
    link.textContent = wallet.address;
    link.href = `${NETWORKS[network].explorer}/contract/${wallet.address}`;
  }
  text("public-balance", `${formatStrk(balance)} STRK`);
  const privateBalance = (notes?.notes.get(BigInt(STRK)) ?? []).reduce(
    (total, n) => total + n.amount,
    0n,
  );
  text(
    "private-balance",
    notes ? `${formatStrk(privateBalance)} STRK` : "Not discovered",
  );
  text(
    "verification",
    info?.checkpoint
      ? `Pool state verified at block ${info.checkpoint.block_number}, against an accepted Starknet RPC header. Cached notes can be older than the current chain head.`
      : "No state verified yet.",
  );
  element("error").hidden = !error;
  text("error", error);
}
async function run(work: () => Promise<void>): Promise<void> {
  if (busy) return;
  busy = true;
  error = "";
  render();
  try {
    await work();
  } catch (e) {
    error = e instanceof Error ? e.message : String(e);
    element<HTMLSelectElement>("network").value = network;
  } finally {
    busy = false;
    render();
  }
}
async function publicState(): Promise<void> {
  if (!transactions) return;
  [balance, deployed] = await Promise.all([
    transactions.balance(),
    transactions.deployed(),
  ]);
}
async function initialize(pageLoad = false): Promise<void> {
  await operations.run(
    wallet ? "Restore wallet and discovery" : "Initialize discovery",
    async () => {
      const previous = provider;
      provider = undefined;
      account = undefined;
      transactions = undefined;
      await previous?.close();
      const config = NETWORKS[network];
      provider = new LocalDiscoveryProvider({
        network,
        feedUrl: config.feedUrl,
        rpcUrl: config.rpc,
        proofRpcUrl: config.proofRpc,
        onEvent: (event) => {
          if (event.event === "span")
            operations.detail(event.value.name, event.value.ms, event.value.bytes);
          if (event.event === "state") {
            info = event.value;
            render();
            // The stream updates pool state; discover this account locally
            // after background updates, without another network round trip.
            if (!busy && account) {
              const current = account;
              void current
                .restore()
                .then((found) => {
                  if (account === current) {
                    notes = found;
                    render();
                  }
                })
                .catch((cause: unknown) => {
                  if (account === current) {
                    error = String(cause);
                    render();
                  }
                });
            }
          }
          if (event.event === "error") {
            error = event.value;
            render();
          }
        },
      });
      info = await provider.ready;
      account = wallet
        ? provider.forAccount({
            address: wallet.address,
            viewingKey: BigInt(wallet.viewingKey),
          })
        : undefined;
      notes = await account?.restore();
      if (pageLoad)
        operations.detail("Navigation to restored discovery result", performance.now());
      transactions = wallet
        ? new Transactions(wallet, provider, operations)
        : undefined;
      render();
    },
  );
  if (transactions)
    await operations.run("Check public wallet balance", publicState);
  if (account && info?.verifiedAt !== null) await provider!.subscribe();
}
async function discover(): Promise<void> {
  if (!wallet || !provider || !transactions) return;
  await operations.run("Discover transaction", async () => {
    const latest = latestPrivateTransaction();
    const block = Math.max(
      await transactions!.rpc.getBlockNumber(),
      latest?.block ?? 0,
    );
    let comparison: Comparison | undefined;
    notes = await observeAt(
      wallet!,
      provider!,
      block,
      element<HTMLInputElement>("compare").checked,
      (result) => {
        comparison = result;
        element("benchmark").replaceChildren(
          ...result.rows.map((row) => {
            const line = document.createElement("p");
            line.className = "benchmark-row";
            line.textContent = `${row.source}: ${(row.ms / 1000).toFixed(2)} s · block ${row.block} · ${row.attempts} attempt(s)${row.error ? ` · Failed: ${row.error}` : ""}`;
            if (!row.retries.length) return line;
            const details = document.createElement("details");
            const summary = document.createElement("summary");
            summary.append(line);
            details.append(summary);
            for (const retry of row.retries) {
              const item = document.createElement("p");
              item.textContent = `${retry.error}: attempt ${(retry.attemptMs / 1000).toFixed(2)} s; ${retry.waitFor === "feed" ? "waiting for feed" : "retry backoff"} ${(retry.waitMs / 1000).toFixed(2)} s`;
              details.append(item);
            }
            return details;
          }),
        );
        if (result.equal !== undefined) {
          const match = document.createElement("p");
          match.textContent = result.equal
            ? "The note sets and spend witnesses match."
            : "The results differ. This run is not a valid speed comparison.";
          element("benchmark").append(match);
        }
      },
    );
    if (comparison) comparisons.push(comparison);
    // SSE can restore notes first, but the explicit observation still records
    // this transaction's timing and optional comparison before the next spend.
    observedTransaction = latest?.hash;
    await provider!.subscribe();
    await publicState();
  });
}
async function next(): Promise<void> {
  const current = step();
  switch (current.action) {
    case "create":
      await operations.run("Create demo wallet", async () => {
        const created = newWallet(network);
        await saveWallet(created);
        wallet = created;
        await initialize();
      });
      break;
    case "initialize":
      await initialize();
      break;
    case "fund":
      await operations.run("Detect wallet funding", async () => {
        await publicState();
        if (balance === 0n)
          throw new Error(
            "No STRK balance yet. Send STRK to the address above, then check again.",
          );
      });
      break;
    case "deploy":
      await transactions!.deploy();
      await publicState();
      break;
    case "resume":
      await operations.run("Confirm pending transaction", async () => {
        await transactions!.resume();
        await publicState();
      });
      break;
    case "discover":
    case "refresh":
      await discover();
      break;
    default: {
      const amount = parseStrk(element<HTMLInputElement>("amount").value);
      const entered = element<HTMLInputElement>("recipient").value.trim();
      const recipient =
        current.action === "withdraw" ? entered : entered || wallet!.address;
      if (
        current.action !== "shield" &&
        !/^0x[0-9a-fA-F]{1,64}$/.test(recipient)
      )
        throw new Error("Enter a Starknet recipient address.");
      await transactions!.execute(
        current.action,
        amount,
        recipient || wallet!.address,
      );
      notes = undefined;
      await publicState();
    }
  }
}
element("next").onclick = () => void run(next);
element("backup").onclick = () => {
  if (wallet) exportWallet(wallet);
};
element("copy").onclick = () =>
  void run(async () => {
    if (wallet) await navigator.clipboard.writeText(wallet.address);
  });
element("import").onclick = () =>
  element<HTMLInputElement>("backup-file").click();
element<HTMLInputElement>("backup-file").onchange = (event) =>
  void run(async () => {
    const file = (event.target as HTMLInputElement).files?.[0];
    if (!file) return;
    if (file.size > 64 * 1024) throw new Error("Wallet backup is too large.");
    const imported = validateWallet(JSON.parse(await file.text()));
    const existing = await loadWallet(imported.network);
    if (existing && BigInt(existing.address) !== BigInt(imported.address))
      throw new Error(
        "A different wallet is already saved for this network. Export it and restore this backup in another browser profile to keep both wallets.",
      );
    await saveWallet(imported);
    localStorage.setItem("strk20-demo-network", imported.network);
    wallet = imported;
    network = wallet.network;
    element<HTMLSelectElement>("network").value = network;
    await initialize();
  });
element<HTMLSelectElement>("network").onchange = () =>
  void run(async () => {
    const selected = element<HTMLSelectElement>("network").value as Network;
    const selectedWallet = await loadWallet(selected);
    localStorage.setItem("strk20-demo-network", selected);
    network = selected;
    wallet = selectedWallet;
    info = undefined;
    notes = undefined;
    balance = 0n;
    deployed = false;
    await initialize();
  });
element("metrics").onclick = () =>
  downloadJson(
    { network, operations: operations.entries, comparisons },
    "strk20-discovery-timings.json",
  );
void run(async () => {
  wallet = await loadWallet(network);
  await initialize(true);
});
