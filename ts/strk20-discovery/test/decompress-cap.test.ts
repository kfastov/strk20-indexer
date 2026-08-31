/**
 * The decompression cap must bound what gets ALLOCATED, not describe what was.
 *
 * Review finding 6: `inflateWithin` used fzstd's one-shot `decompress()` and
 * compared `out.length > cap` afterwards. `decompress()` reads the zstd frame
 * header's declared content size and allocates exactly that before returning,
 * so the comparison ran after the damage. A hostile feed only has to publish a
 * manifest whose `zst` hash matches the bomb — the upstream hash check passes,
 * because the bomb IS the bytes it published.
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { decompress } from 'fzstd';

import { inflateWithin } from '../src/client.ts';
import { Strk20Error } from '../src/errors.ts';

/**
 * A minimal, hand-built zstd frame whose header DECLARES `declared` bytes of
 * content while actually carrying `content` in one raw block.
 *
 * Layout (RFC 8878 §3.1.1): magic, Frame_Header_Descriptor, Window_Descriptor,
 * Frame_Content_Size, then blocks. The descriptor below sets
 * Frame_Content_Size_flag=2 (a 4-byte size), Single_Segment_flag=0 (so a window
 * descriptor follows and the window, not the declared size, is what a streaming
 * decoder has to allocate), no dictionary and no checksum.
 */
function frameDeclaring(declared: number, content: Uint8Array): Uint8Array {
  const blockHeader = 1 /* last block */ | (0 << 1) /* raw */ | (content.length << 3);
  const head = Uint8Array.from([
    0x28, 0xb5, 0x2f, 0xfd, // magic
    0x80, // FCS_flag=2, single_segment=0, no checksum, no dict id
    0x08, // window descriptor: 2 KiB
    declared & 255, (declared >>> 8) & 255, (declared >>> 16) & 255, (declared >>> 24) & 255,
    blockHeader & 255, (blockHeader >>> 8) & 255, (blockHeader >>> 16) & 255,
  ]);
  const out = new Uint8Array(head.length + content.length);
  out.set(head, 0);
  out.set(content, head.length);
  return out;
}

const CONTENT = Uint8Array.from({ length: 100 }, (_, i) => i & 255);
const DECLARED = 4 * 1024 * 1024;
const CAP = 64 * 1024;

test('a frame that LIES about its size is decoded, not allocated', () => {
  const bomb = frameDeclaring(DECLARED, CONTENT);

  // The premise, asserted rather than assumed: fzstd's one-shot decoder — the
  // call the old implementation made — honours the header's claim and produces
  // 4 MiB from 100 bytes of content. Under the old code this was allocated
  // BEFORE `out.length > cap` was ever evaluated.
  assert.equal(decompress(bomb).length, DECLARED, 'the one-shot decoder allocates the declared size');

  // The fix: the streamed decode is bounded by what actually comes out.
  const got = inflateWithin(bomb, CAP, '/epochs/00000000.strk20e.zst');
  assert.equal(got.length, CONTENT.length);
  assert.deepEqual(got, CONTENT);
});

test('output that really does exceed the cap is refused by name', () => {
  const big = Uint8Array.from({ length: 2000 }, (_, i) => i & 255);
  const frame = frameDeclaring(big.length, big);
  assert.throws(
    () => inflateWithin(frame, 1000, '/snapshots/00000000.zst'),
    (e: unknown) => e instanceof Strk20Error && e.code === 'DECOMPRESS_LIMIT',
  );
});

test('a well-formed artifact inside its cap round-trips', () => {
  const frame = frameDeclaring(CONTENT.length, CONTENT);
  assert.deepEqual(inflateWithin(frame, CAP, '/epochs/00000001.strk20e.zst'), CONTENT);
});
