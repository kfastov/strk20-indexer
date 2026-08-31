/**
 * §A6 chain profiles. One profile source, consumed by Rust and TypeScript.
 *
 * These values are read from the feeds this repo actually publishes
 * (`data/mainnet/feed/genesis.json`) and from live-run-findings.md §5 for
 * Sepolia. They are the identity the client PINS BEFORE a byte is requested,
 * which is what closes the trust-on-first-use hole: an empty mirror must not
 * adopt whatever chain the feed declares (§3.10 item 3).
 */
import { Strk20Error } from "./errors.js";
export const MAINNET = Object.freeze({
    name: 'mainnet',
    chainId: 'SN_MAIN',
    pool: '0x40337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a',
    genesisBlock: 8978970,
    epochSize: 10000,
    feedFormat: 1,
});
export const SEPOLIA = Object.freeze({
    name: 'sepolia',
    chainId: 'SN_SEPOLIA',
    pool: '0x0254a6b2997ef52e9f830ce1f543f6b29768295e8d17e2267d672c552cfe0d91',
    genesisBlock: 8271125,
    epochSize: 10000,
    feedFormat: 1,
});
export function resolveProfile(n) {
    if (n === undefined || n === 'mainnet')
        return MAINNET;
    if (n === 'sepolia')
        return SEPOLIA;
    if (typeof n === 'object' && typeof n.chainId === 'string' && typeof n.pool === 'string')
        return n;
    throw new Strk20Error('CONFIG_INVALID', 'unknown network', { option: 'network', got: String(n) });
}
//# sourceMappingURL=profiles.js.map