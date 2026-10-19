/**
 * Expected feed identities, pinned before downloading data. Keep these
 * consistent with the supported Rust network profiles in config.rs; an empty
 * client must not adopt whichever chain an arbitrary feed declares.
 */

import type { ChainProfile } from "./types.ts";

export const MAINNET: ChainProfile = Object.freeze({
  name: "mainnet",
  chainId: "SN_MAIN",
  pool: "0x40337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a",
  genesisBlock: 8978970,
  epochSize: 10000,
  feedFormat: 1,
});

export const SEPOLIA: ChainProfile = Object.freeze({
  name: "sepolia",
  chainId: "SN_SEPOLIA",
  pool: "0x0254a6b2997ef52e9f830ce1f543f6b29768295e8d17e2267d672c552cfe0d91",
  genesisBlock: 8271125,
  epochSize: 10000,
  feedFormat: 1,
});

export function resolveProfile(
  n: "mainnet" | "sepolia" | ChainProfile | undefined,
): ChainProfile {
  if (n === undefined || n === "mainnet") return MAINNET;
  if (n === "sepolia") return SEPOLIA;
  if (
    typeof n === "object" &&
    typeof n.chainId === "string" &&
    typeof n.pool === "string"
  )
    return n;
  throw new Error("CONFIG_INVALID: unknown network");
}
