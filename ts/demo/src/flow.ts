import type { Action, Wallet } from "./wallet.ts";

export type WalletAction = Exclude<Action, "deploy"> | "discover";
export function latestTransaction(wallet: Pick<Wallet, "completed">) {
  return Object.entries(wallet.completed)
    .filter(([action]) => action !== "deploy")
    .map(([action, record]) => ({ ...record, action: action as Exclude<Action, "deploy"> }))
    .sort((a, b) => b.block - a.block)[0];
}

const descriptions = {
  shield: { label: "Deposit STRK", description: "Move public STRK into the privacy pool. You can repeat deposits with this wallet." },
  transfer: { label: "Transfer privately", description: "Send to yourself for a complete demo, or enter another registered private recipient." },
  withdraw: { label: "Withdraw STRK", description: "Return private funds to a public wallet. Keep public STRK for gas and the pool fee." },
  discover: { label: "Discover transaction", description: "Verify pool state and refresh your private balance locally." },
};

/** Completed records suggest the next operation; they never lock it out. */
export function walletStep(wallet: Pick<Wallet, "completed" | "pending">,
  observed: string | undefined, selected?: WalletAction) {
  if (wallet.pending) return { action: "resume" as const, label: "Resume pending transaction",
    description: "Check the existing transaction before sending another one." };
  const latest = latestTransaction(wallet);
  const action = selected ?? (latest && observed !== latest.hash ? "discover"
    : latest?.action === "shield" ? "transfer"
    : latest?.action === "transfer" ? "withdraw" : "shield");
  return { action, ...descriptions[action] };
}
