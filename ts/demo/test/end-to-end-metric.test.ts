/**
 * Issue #22 — the REPLAY lane printed `end-to-end 14180155 s`.
 *
 * The old line was `Date.now() - hit.blockTimestamp * 1000`, unconditionally,
 * under a `measured` badge. The arithmetic was right and the input was not:
 * the REPLAY feed's block timestamps are a linear function of the block number
 * fixed when `scripts/gen-replay-feed.mjs` ran, so the subtraction printed the
 * fixture's age, and the wasm engine reports no timestamp at all.
 *
 * The two inputs below are copied from the generator and from the adapter, so
 * the numbers this test refuses are the numbers the demo actually produced.
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { endToEndMetric } from '../src/state.ts';

/**
 * `gen-replay-feed.mjs`: `ts: 1756000000 + (b - GENESIS_BLOCK) * 3`, with
 * GENESIS_BLOCK 8271125 and the note at NOTE_BLOCK 14339115.
 */
const REPLAY_NOTE_TS = 1756000000 + (14339115 - 8271125) * 3;
/** The wall clock at which the demo printed the number in the issue. */
const REPORTED_AT_MS = (REPLAY_NOTE_TS + 14180155) * 1000;

test('the replay fixture reproduces the reported number, and it is refused', () => {
  // The regression, spelled out: this is where `14180155 s` came from.
  assert.equal(Math.round(REPORTED_AT_MS / 1000 - REPLAY_NOTE_TS), 14180155);

  const m = endToEndMetric(REPLAY_NOTE_TS, REPORTED_AT_MS, false);
  assert.equal(m.provenance, 'unavailable');
  assert.equal(m.value, 'unavailable');
});

test('the wasm engine reports no timestamp, so the slot stays empty', () => {
  // `engine-wasm.ts` hardcodes `blockTimestamp: 0`; the old line turned that
  // into the Unix epoch printed as seconds.
  const m = endToEndMetric(0, REPORTED_AT_MS, true);
  assert.equal(m.provenance, 'unavailable');
  assert.equal(m.value, 'unavailable');
});

test('a real chain clock yields the elapsed seconds, rounded', () => {
  const m = endToEndMetric(REPLAY_NOTE_TS, REPLAY_NOTE_TS * 1000 + 41_600, true);
  assert.equal(m.provenance, 'measured');
  assert.equal(m.value, '42 s');
  assert.equal(m.label, 'end-to-end');
});

test('a block ahead of the local clock is skew, not a negative latency', () => {
  const m = endToEndMetric(REPLAY_NOTE_TS, REPLAY_NOTE_TS * 1000 - 5_000, true);
  assert.equal(m.provenance, 'unavailable');
});
