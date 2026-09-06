import { constants } from "starknet";

export type Network = "mainnet" | "sepolia";
export const STRK =
  "0x04718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d";
export const ACCOUNT_CLASS =
  "0x05b4b537eaa2399e3aa99c4e2e0208ebd6c71bc1467938cd52c798c601e43564";
export const NETWORKS = {
  mainnet: {
    chainId: constants.StarknetChainId.SN_MAIN,
    pool: "0x040337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a",
    rpc: "https://starknet.publicnode.com",
    ws: "wss://starknet-rpc.publicnode.com",
    proofRpc: "https://rpc.starknet.lava.build",
    feedUrl: "https://strk20.nullref.cc/mainnet/feed",
    prover: "https://transaction-prover.alpha-mainnet.sw-dev.io",
    reference: "https://discovery-service.alpha-mainnet.sw-dev.io",
    explorer: "https://voyager.online",
  },
  sepolia: {
    chainId: constants.StarknetChainId.SN_SEPOLIA,
    pool: "0x0254a6b2997ef52e9f830ce1f543f6b29768295e8d17e2267d672c552cfe0d91",
    rpc: "https://starknet-sepolia-rpc.publicnode.com",
    ws: "wss://starknet-sepolia-rpc.publicnode.com",
    proofRpc: "https://api.cartridge.gg/x/starknet/sepolia",
    feedUrl: "https://strk20.nullref.cc/feed",
    prover: "https://transaction-prover.alpha-sepolia.sw-dev.io",
    reference: "https://discovery-service.alpha-sepolia.sw-dev.io",
    explorer: "https://sepolia.voyager.online",
  },
} as const;

export function formatStrk(value: bigint): string {
  const fraction = (value % 10n ** 18n)
    .toString()
    .padStart(18, "0")
    .slice(0, 5);
  return `${value / 10n ** 18n}.${fraction}`;
}

export function parseStrk(value: string): bigint {
  if (!/^\d+(\.\d{1,18})?$/.test(value))
    throw new Error("Enter a STRK amount with at most 18 decimal places.");
  const [whole, fraction = ""] = value.split(".");
  const result = BigInt(whole!) * 10n ** 18n + BigInt(fraction.padEnd(18, "0"));
  if (result <= 0n) throw new Error("The amount must be positive.");
  return result;
}
