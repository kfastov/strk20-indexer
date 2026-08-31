import type { Account } from './types.ts';
/**
 * For a backend or CLI that legitimately holds the bytes for the process
 * lifetime. Named so the shape is visible in the integrator's own review.
 *
 * This holds a JS buffer that cannot be reliably zeroized. The guarantee this
 * package makes is NON-TRANSMISSION, not host memory hygiene — see the README
 * paragraph that says the module never writes a key anywhere.
 */
export declare function staticAccount(address: `0x${string}`, key: Uint8Array): Account;
export declare function assertKey(key: Uint8Array): void;
export declare function assertAddress(a: string): asserts a is `0x${string}`;
/** Zeroize a buffer we were handed. Called on every path out of a pass. */
export declare function zeroize(b: Uint8Array | null | undefined): void;
//# sourceMappingURL=account.d.ts.map