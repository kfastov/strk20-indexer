/**
 * HMAC-SHA256 and HKDF over the synchronous sha256, used for exactly one thing:
 * §4.4's `keyId`.
 *
 *   keyId = hex(HKDF-SHA256(ikm = viewingKey,
 *                           salt = "strk20-idb-keyid-v1",
 *                           info = chain_id ‖ pool ‖ owner))
 *
 * The FULL 32-byte output rendered as 64 lowercase hex characters, no slice.
 * Unguessable without the key, which is what keeps an IndexedDB row from being
 * an identifier anyone else can compute.
 *
 * Synchronous because it runs inside a discovery pass, beside the engine, on a
 * key that is zeroized when the pass returns; a Promise here would mean holding
 * the key across a microtask boundary for no benefit.
 */
export declare function hmacSha256(key: Uint8Array, message: Uint8Array): Uint8Array;
/** One-block HKDF (32 bytes out), which is all `keyId` needs. */
export declare function hkdf32(ikm: Uint8Array, salt: Uint8Array, info: Uint8Array): Uint8Array;
export declare function keyId(viewingKey: Uint8Array, chainId: string, pool: string, owner: string): string;
//# sourceMappingURL=kdf.d.ts.map