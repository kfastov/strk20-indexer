import { NETWORKS, type Network } from "./network.ts";

export const rpcSettingsKey = (network: Network) => `strk20-demo-rpc-${network}`;

/** A user-owned QuickNode URL stays in this browser, never in the public build. */
export function parseRpcSettings(network: Network, value: string) {
  if (!value.trim()) return undefined;
  const url = new URL(value.trim());
  if (url.protocol !== "https:" || !url.hostname.endsWith(`.strk-${network}.quiknode.pro`) ||
      url.username || url.password || url.port || url.search || url.hash ||
      !/^\/[a-zA-Z0-9_-]+(?:\/rpc\/v0_9)?\/?$/.test(url.pathname)) {
    throw new Error(`Enter the HTTPS QuickNode URL for Starknet ${network}.`);
  }
  url.pathname = url.pathname.replace(/\/$/, "").replace(/\/rpc\/v0_9$/, "") + "/rpc/v0_9";
  const rpc = url.href;
  url.protocol = "wss:";
  return { rpc, ws: url.href, proofRpc: rpc };
}

export function networkConfig(network: Network) {
  const saved = typeof window === "undefined" ? "" : localStorage.getItem(rpcSettingsKey(network)) ?? "";
  return { ...NETWORKS[network], ...parseRpcSettings(network, saved) };
}
