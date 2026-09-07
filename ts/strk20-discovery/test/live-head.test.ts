import assert from "node:assert/strict";
import { test } from "node:test";
import { LiveHeadAssembler, type LiveHead } from "../src/live-head.ts";

const header = (head: number) => `{"t":"hdr","head":${head}}\n`;
const end = '{"t":"end"}\n';
const record = '{"t":"blk","number":10}\n';
const base: LiveHead = { head: 10, head_hash: "0xa", l1_accepted: 9,
  tail_from: 1, etag: "head-10", payload: header(10) + record + end, resync: false };
const delta = (from: number, to: number, append = ""): LiveHead => ({
  ...base, head: to, etag: `head-${to}`, payload: null,
  delta: { base_etag: `head-${from}`, header: header(to), append, end },
});

test("consecutive deltas reconstruct canonical bytes even when verification coalesces heads", () => {
  const heads = new LiveHeadAssembler();
  heads.receive(base);
  assert.equal(heads.receive(delta(10, 11)).payload, header(11) + record + end);
  const added = '{"t":"blk","number":12}\n';
  assert.equal(heads.receive(delta(11, 12, added)).payload, header(12) + record + added + end);
  assert.equal(heads.receive(delta(12, 15)).payload, header(15) + record + added + end);
  const replacement = { ...base, head: 9, etag: "head-9", payload: header(9) + end };
  assert.equal(heads.receive(replacement).payload, replacement.payload, "reorg replaces old records");
  assert.equal(heads.receive(delta(9, 10)).payload, header(10) + end);
});

test("missing base, reconnect, wrong epoch and oversized accumulation cannot apply a delta", () => {
  const heads = new LiveHeadAssembler();
  assert.throws(() => heads.receive(delta(10, 11)), /FEED_LIVE_GAP/);
  heads.receive(base);
  assert.throws(() => heads.receive(delta(11, 12)), /FEED_LIVE_GAP/);
  heads.receive(base);
  assert.throws(() => heads.receive({ ...delta(10, 11), tail_from: 11 }), /FEED_LIVE_GAP/);
  heads.receive(base);
  assert.throws(() => heads.receive(delta(10, 11, "x".repeat(2 * 1024 * 1024) + "\n")), /FEED_LIVE_GAP/);
  heads.receive(base);
  heads.reset();
  assert.throws(() => heads.receive(delta(10, 11)), /FEED_LIVE_GAP/);
  heads.receive(base);
  assert.equal(heads.receive(delta(10, 11)).head, 11);
});
