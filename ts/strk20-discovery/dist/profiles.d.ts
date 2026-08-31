/**
 * §A6 chain profiles. One profile source, consumed by Rust and TypeScript.
 *
 * These values are read from the feeds this repo actually publishes
 * (`data/mainnet/feed/genesis.json`) and from live-run-findings.md §5 for
 * Sepolia. They are the identity the client PINS BEFORE a byte is requested,
 * which is what closes the trust-on-first-use hole: an empty mirror must not
 * adopt whatever chain the feed declares (§3.10 item 3).
 */
import type { ChainProfile } from './types.ts';
export declare const MAINNET: ChainProfile;
export declare const SEPOLIA: ChainProfile;
export declare function resolveProfile(n: 'mainnet' | 'sepolia' | ChainProfile | undefined): ChainProfile;
//# sourceMappingURL=profiles.d.ts.map