import { Decompress } from "fzstd";

export function inflateWithin(
  bytes: Uint8Array,
  cap = 256 * 1024 * 1024,
): Uint8Array {
  checkFrame(bytes, cap);
  const parts: Uint8Array[] = [];
  let length = 0;
  const decoder = new Decompress((chunk) => {
    length += chunk.length;
    if (length > cap) throw new Error("DECOMPRESS_LIMIT");
    parts.push(chunk);
  });
  // Avoid passing a malicious frame's claimed size to a whole-frame allocator.
  for (let offset = 0; offset < bytes.length; offset += 16_384)
    decoder.push(
      bytes.subarray(offset, offset + 16_384),
      offset + 16_384 >= bytes.length,
    );
  const out = new Uint8Array(length);
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}

// Validate the single canonical zstd frame before fzstd allocates its window.
function checkFrame(bytes: Uint8Array, cap: number): void {
  const fail = () => {
    throw new Error("DECOMPRESS_LIMIT: invalid or oversized zstd frame");
  };
  if (
    bytes.length < 6 ||
    bytes[0] !== 0x28 ||
    bytes[1] !== 0xb5 ||
    bytes[2] !== 0x2f ||
    bytes[3] !== 0xfd
  )
    fail();
  const flag = bytes[4]!,
    single = (flag & 32) !== 0,
    sizeFlag = flag >>> 6;
  if (flag & 24) fail();
  const window = single
    ? 0
    : 2 ** (10 + (bytes[5]! >>> 3)) * (1 + (bytes[5]! & 7) / 8);
  let at = single ? 5 : 6;
  at += [0, 1, 2, 4][flag & 3]!;
  const count = sizeFlag ? 2 ** sizeFlag : single ? 1 : 0;
  if (at + count > bytes.length) fail();
  let size = 0n;
  for (let i = 0; i < count; i++)
    size += BigInt(bytes[at + i]!) << BigInt(8 * i);
  if (sizeFlag === 1) size += 256n;
  if ((single && size > BigInt(cap)) || window > cap) fail();
  at += count;
  for (;;) {
    if (at + 3 > bytes.length) fail();
    const header = bytes[at]! | (bytes[at + 1]! << 8) | (bytes[at + 2]! << 16);
    at += 3;
    const kind = (header >>> 1) & 3;
    if (kind === 3) fail();
    at += kind === 1 ? 1 : header >>> 3;
    if (at > bytes.length) fail();
    if (header & 1) break;
  }
  if (flag & 4) at += 4;
  if (at !== bytes.length) fail();
}
