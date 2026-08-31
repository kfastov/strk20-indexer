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

import { concatBytes, sha256, toHex } from './sha256.ts';

const BLOCK = 64;

export function hmacSha256(key: Uint8Array, message: Uint8Array): Uint8Array {
  let k = key;
  if (k.length > BLOCK) k = sha256(k);
  const padded = new Uint8Array(BLOCK);
  padded.set(k);
  const ipad = new Uint8Array(BLOCK);
  const opad = new Uint8Array(BLOCK);
  for (let i = 0; i < BLOCK; i++) {
    ipad[i] = padded[i]! ^ 0x36;
    opad[i] = padded[i]! ^ 0x5c;
  }
  const inner = sha256(concatBytes(ipad, message));
  const out = sha256(concatBytes(opad, inner));
  padded.fill(0);
  ipad.fill(0);
  opad.fill(0);
  return out;
}

/** One-block HKDF (32 bytes out), which is all `keyId` needs. */
export function hkdf32(ikm: Uint8Array, salt: Uint8Array, info: Uint8Array): Uint8Array {
  const prk = hmacSha256(salt, ikm);
  const okm = hmacSha256(prk, concatBytes(info, Uint8Array.of(1)));
  prk.fill(0);
  return okm;
}

const enc = new TextEncoder();

export function keyId(viewingKey: Uint8Array, chainId: string, pool: string, owner: string): string {
  const okm = hkdf32(viewingKey, enc.encode('strk20-idb-keyid-v1'), enc.encode(chainId + pool + owner));
  const hex = toHex(okm);
  okm.fill(0);
  return hex;
}
