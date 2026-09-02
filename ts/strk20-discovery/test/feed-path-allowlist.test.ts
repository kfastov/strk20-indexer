/**
 * The closed feed-path allowlist, checked against the one authority for it:
 * `PATTERNS` in `crates/e2e-tests/src/feed_urls.rs`, which is what the server
 * enforces. The client's list is the same nine with the `/feed` mount point
 * stripped, because that part lives in the base URL here.
 *
 * The defect this pins: the two snapshot artifacts were spelled
 * `/snapshot.strk20s.zst` and `/snapshot.anchor.json` — singular, no
 * directory, no epoch index — while every server writes, and the manifest's
 * `snapshot.file` names, `snapshots/{e:08}.…`. Nothing caught it because no
 * test ever fed this list a snapshot path, and a feed without a snapshot never
 * asks for one. On the public mainnet feed the browser demo got as far as
 * genesis.json and manifest.json and then threw
 * `SCOPE_VIOLATION: path is not in the closed feed allowlist`.
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { FEED_PATH_ALLOWLIST, isAllowedFeedPath } from '../src/net.ts';

/** feed_urls.rs PATTERNS, minus `/feed`, with the indices filled in. */
const ADMITTED = [
  '/genesis.json',
  '/manifest.json',
  '/head.ndjson',
  '/anchors.ndjson',
  '/live',
  '/epochs/00001425.strk20e.zst',
  '/epochs/00001425.anchor.json',
  '/snapshots/00001425.strk20s.zst',
  '/snapshots/00001425.anchor.json',
];

test('every path the server publishes is admitted', () => {
  for (const p of ADMITTED) assert.equal(isAllowedFeedPath(p), true, p);
  assert.equal(FEED_PATH_ALLOWLIST.length, ADMITTED.length);
});

test('the snapshot artifacts the wasm planner asks for are admitted', () => {
  // engine-wasm.ts builds these two from `manifest.snapshot.file` and
  // `manifest.snapshot.e`; the shapes are consumer-path.md §C8 and §706.
  const e = String(1425).padStart(8, '0');
  assert.equal(isAllowedFeedPath(`/snapshots/${e}.strk20s.zst`), true);
  assert.equal(isAllowedFeedPath(`/snapshots/${e}.anchor.json`), true);
});

test('a query string is unmatched, not merely forbidden', () => {
  for (const p of [
    '/manifest.json?key=0x1',
    '/snapshots/00001425.strk20s.zst?vk=0x1',
    '/epochs/00001425.strk20e.zst?',
    '/live?address=0x1',
  ]) {
    assert.equal(isAllowedFeedPath(p), false, p);
  }
});

test('traversal, prefixes and unpadded indices are rejected', () => {
  for (const p of [
    '/snapshots/../../etc/passwd',
    '/snapshots/00001425.strk20s.zst/../manifest.json',
    '/snapshots/1425.strk20s.zst',
    '/snapshots/000014250.strk20s.zst',
    '/snapshots/',
    '/snapshots/latest.strk20s.zst',
    // The spelling this list carried before, which named nothing on any server.
    '/snapshot.strk20s.zst',
    '/snapshot.anchor.json',
    '/v1/raw/0x1',
    '/manifest.json/extra',
    'manifest.json',
  ]) {
    assert.equal(isAllowedFeedPath(p), false, p);
  }
});
