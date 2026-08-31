/**
 * Synchronous SHA-256.
 *
 * Why a hand-rolled one rather than WebCrypto: the engine ABI (§3.3) is
 * SYNCHRONOUS — bytes in, notes out, no async inside the computer — and
 * `crypto.subtle.digest` is a Promise. The wasm engine hashes in Rust; the mock
 * engine needs the same shape in TypeScript to be a faithful stand-in.
 *
 * This is a verification-side hash over public feed bytes. It never touches a
 * viewing key.
 */
export declare function sha256(data: Uint8Array): Uint8Array;
export declare function toHex(bytes: Uint8Array): string;
export declare function fromHex(hex: string): Uint8Array;
export declare function sha256Hex(data: Uint8Array): string;
export declare function sha256HexOfString(s: string): string;
export declare function concatBytes(...parts: Uint8Array[]): Uint8Array;
//# sourceMappingURL=sha256.d.ts.map