/**
 * The simulated feed.
 *
 * Shapes are copied from the real `data/mainnet/feed` in this repo so the URLs
 * and byte counts in the network panel are the ones a real run would produce:
 *
 *   genesis.json        194 B
 *   manifest.json   146,181 B      (518 epoch entries)
 *   epochs/*     15,817,408 B      (897 .. 1414, 518 files)
 *   head.ndjson      58,583 B
 *   anchors.ndjson      259 B
 *                 ------------
 *                 16,022,366 B  ≈  16.0 MB, 521 GETs
 *
 * The per-epoch sizes are generated from a seeded curve pinned to the real
 * endpoints (2,472 B at epoch 897, 37,711 B at epoch 1414) and normalised so
 * the total matches exactly. They are plausible, not measured.
 *
 * Nothing in here is fetched. There is no network in this prototype at all —
 * which is itself worth knowing when you read the network panel.
 */

import { makeRng } from './latency';
import type { Felt, Identity } from './types';

export const CHAIN_ID = 'SN_MAIN';

/** The deployed mainnet pool, from README.md. Public, in every URL-free sense. */
export const POOL: Felt = '0x040337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a';

export const GENESIS_BLOCK = 8_978_970;
export const EPOCH_SIZE = 10_000;

export const FIRST_EPOCH = 897;
export const LAST_EPOCH = 1414;
export const EPOCH_COUNT = LAST_EPOCH - FIRST_EPOCH + 1; // 518

export const HEAD_BLOCK = 14_159_049;
export const L1_ACCEPTED = 14_150_775;

export const GENESIS_BYTES = 194;
export const MANIFEST_BYTES = 146_181;
export const HEAD_BYTES = 58_583;
export const ANCHORS_BYTES = 259;
const EPOCH_BYTES_TOTAL = 15_817_408;

/** The epoch a snapshot would be cut at, if snapshots existed. See notes below. */
export const SNAPSHOT_EPOCH = 1414;

export function epochPath(e: number): string {
  return `/feed/epochs/${String(e).padStart(8, '0')}.strk20e.zst`;
}

export function snapshotPath(e: number): string {
  return `/feed/snapshots/${String(e).padStart(8, '0')}.strk20s.zst`;
}

export function snapshotAnchorPath(e: number): string {
  return `/feed/snapshots/${String(e).padStart(8, '0')}.anchor.json`;
}

/** Per-epoch compressed sizes, deterministic, summing to EPOCH_BYTES_TOTAL. */
export const EPOCH_BYTES: readonly number[] = (() => {
  const rng = makeRng(0x5721_0e00);
  const raw: number[] = [];
  for (let i = 0; i < EPOCH_COUNT; i++) {
    const t = i / (EPOCH_COUNT - 1);
    // Pool activity grew over time; the real files do too (2.4 kB -> 37.7 kB).
    const trend = 2_472 + (37_711 - 2_472) * Math.pow(t, 1.35);
    const noise = 0.55 + rng() * 0.95;
    raw.push(trend * noise);
  }
  const sum = raw.reduce((a, b) => a + b, 0);
  const scale = EPOCH_BYTES_TOTAL / sum;
  const out = raw.map((v) => Math.max(600, Math.round(v * scale)));
  // Absorb rounding drift into the last entry so the total is exact.
  const drift = EPOCH_BYTES_TOTAL - out.reduce((a, b) => a + b, 0);
  out[out.length - 1] = (out[out.length - 1] ?? 0) + drift;
  return out;
})();

export function epochBytes(e: number): number {
  return EPOCH_BYTES[e - FIRST_EPOCH] ?? 6_000;
}

/**
 * The snapshot lane's byte cost is deliberately NOT invented here. No snapshot
 * has ever been cut: `manifest.snapshot` is `null` in the real feed and the
 * cutter that would produce one is roadmap item 1. The request count is
 * arithmetic from the spec (§1.7: genesis + manifest + snapshot + anchor +
 * 0-1 epochs + head); the byte count would be a fabrication, so the panel
 * prints "unmeasured" instead of a number.
 */
export const SNAPSHOT_LANE_REQUESTS = 6;

// ---------------------------------------------------------------------------
// identities
// ---------------------------------------------------------------------------

export const IDENTITIES: Readonly<Record<'A' | 'B', Identity>> = {
  A: {
    id: 'A',
    label: 'wallet A',
    address: '0x34ba56f92265f0868c57d3fe72ecab144fc96f97954bbbc4252cce7d0f5fd1c',
    viewingKey: '0xa11ce',
  },
  B: {
    id: 'B',
    label: 'wallet B',
    address: '0x2939f2dcd2b9c78e2c56a37e4c3d1a90e59e7cc16d64de3ec2b6e0d3d18a55c',
    viewingKey: '0xb0b',
  },
};

/**
 * Starting notes per wallet, so switching identity shows a genuinely different
 * discovery result over an identical request list. Wallet B starts empty on
 * purpose: it makes the "same bytes, different answer" point sharper, and it
 * exercises the send/withdraw gates from zero.
 */
export const SEED_NOTES: Readonly<Record<'A' | 'B', ReadonlyArray<{ amount: number; block: number }>>> = {
  A: [
    { amount: 3.0, block: 13_402_118 },
    { amount: 12.5, block: 14_005_331 },
  ],
  B: [],
};

/** Deterministic 0x-hex of `n` nibbles from a label, for readable fake felts. */
export function fakeFelt(label: string, nibbles = 63): Felt {
  let h = 0x811c9dc5;
  for (let i = 0; i < label.length; i++) {
    h ^= label.charCodeAt(i);
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  const rng = makeRng(h);
  let s = '';
  while (s.length < nibbles) s += Math.floor(rng() * 16).toString(16);
  return `0x${s.slice(0, nibbles).replace(/^0+/, '') || '0'}`;
}
