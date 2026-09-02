/**
 * Felt equality over hex strings.
 *
 * `0x0254a6b…` and `0x254a6b…` are the SAME address. The profiles in
 * `profiles.ts` are written zero-padded to 64 nibbles; the feeds this repo
 * actually publishes (`data/sepolia/idx/feed/genesis.json`,
 * `data/mainnet/feed/genesis.json`) emit the unpadded spelling, and a feed is
 * under no obligation to pick the spelling the profile did. Comparing the
 * STRINGS rejects an honest feed with `CHAIN_MISMATCH`, which is the loudest
 * possible failure for a difference that carries no meaning.
 *
 * This lives in its own module because both engines have to answer the
 * question the same way. It used to be a private function in `engine-wasm.ts`
 * while `engine-mock.ts` compared strings, so the two engines disagreed about
 * whether the published Sepolia feed was the chain the caller had pinned. Two
 * copies of a trust rule is one copy too many, and the direction of the error
 * matters: this must never make two DIFFERENT felts compare equal, only two
 * spellings of one.
 */

const HEX = /^0x[0-9a-fA-F]+$/;

export function feltEq(a: unknown, b: unknown): boolean {
  if (typeof a !== 'string' || typeof b !== 'string') return false;
  if (!HEX.test(a.trim()) || !HEX.test(b.trim())) return false;
  return norm(a) === norm(b);
}

/** Lower-case, `0x` and leading zeros stripped. `0x0` normalises to the empty string, and so does `0x00`. */
function norm(s: string): string {
  return s.trim().toLowerCase().replace(/^0x/, '').replace(/^0+/, '');
}
