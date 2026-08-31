#!/usr/bin/env node
/**
 * Generates the REPLAY feed fixture.
 *
 * HONESTY NOTE, and it is the first thing the demo says about this lane:
 * demo-app.md §3 specifies REPLAY as "a pinned static feed directory CAPTURED
 * from Sepolia at a named manifest hash". This tree contains no such capture —
 * `data/sepolia` holds SQLite databases, not a cut feed — so what this script
 * produces is a SYNTHETIC fixture shaped like one. It carries the real block
 * numbers from live-run-findings.md §5 (the note at 14,339,115, the spend at
 * 14,340,785) and the real feed document schema, and nothing else about it is a
 * capture. The demo labels the lane SYNTHETIC everywhere, and §10's
 * "REPLAY provenance is a shipping prerequisite" remains open.
 *
 * Three stages, each a complete static feed directory, so the demo advances the
 * recorded history by changing its feed BASE — never by adding a query string,
 * which the URL allowlist makes unrepresentable:
 *
 *   t0  epochs 827..1432 + a tail that stops BEFORE the note
 *   t1  the same epochs  + a tail that INCLUDES the note        (deposit lands)
 *   t2  epochs 827..1433 + a tail that includes the spend       (note is spent)
 *
 * t0 → t1 changes only head.ndjson, which is exactly the reload delta the spec
 * describes; t1 → t2 additionally cuts one epoch, so the demo shows an epoch
 * boundary being crossed.
 *
 * Deterministic: one seeded PRNG, no clock, no randomness. Re-running it
 * produces byte-identical output, which is what makes the CI lane meaningful.
 */

import { createHash } from 'node:crypto';
import { mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { zstdCompressSync } from 'node:zlib';

const here = fileURLToPath(new URL('.', import.meta.url));
const OUT = join(here, '..', 'public', 'replay');
const IDS = JSON.parse(readFileSync(join(here, '..', 'fixtures', 'replay-identities.json'), 'utf8'));

const CHAIN_ID = 'SN_SEPOLIA';
const POOL = '0x0254a6b2997ef52e9f830ce1f543f6b29768295e8d17e2267d672c552cfe0d91';
const GENESIS_BLOCK = 8271125;
const EPOCH_SIZE = 10000;

// From live-run-findings.md §5, the two blocks of our own Sepolia transactions.
const NOTE_BLOCK = 14339115;
const SPEND_BLOCK = 14340785;
const NOTE_ID = '0xce526b286fed962b9e3942771c5e519c69b8677dc24136ae380ba523a067ff';
const TOKEN_STRK = '0x4718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d';

const small = process.argv.includes('--small');
// Sepolia's real shape (live-run §5): 19,030 events over 4,455 pool-active
// blocks, 606 epochs. `--small` exists for the test suite, not for the demo.
const ACTIVE_BLOCKS = small ? 200 : 4455;
const MOCK_NOTES = small ? 120 : 3000;

const sha = (b) => createHash('sha256').update(b).digest('hex');
const enc = (s) => Buffer.from(s, 'utf8');

function tagFor(keyHex, noteId) {
  return createHash('sha256')
    .update(Buffer.concat([enc('strk20-mock-tag/'), Buffer.from(keyHex, 'hex'), enc(noteId)]))
    .digest('hex')
    .slice(0, 16);
}
function nullifierFor(keyHex, noteId) {
  return (
    '0x' +
    createHash('sha256')
      .update(Buffer.concat([enc('strk20-mock-nf/'), Buffer.from(keyHex, 'hex'), enc(noteId)]))
      .digest('hex')
      .slice(0, 62)
  );
}

// mulberry32 — small, deterministic, and not pretending to be a CSPRNG.
function rng(seed) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}
const rand = rng(0x5417c20);
const felt = () => '0x' + Array.from({ length: 62 }, () => '0123456789abcdef'[(rand() * 16) | 0]).join('');

// ------------------------------------------------------------- build history

const FIRST_EPOCH = Math.floor(GENESIS_BLOCK / EPOCH_SIZE); // 827
const LAST_BLOCK = 14340900;
const LAST_EPOCH = Math.floor(LAST_BLOCK / EPOCH_SIZE); // 1434

/** Pool-active blocks, spread over the whole range so every epoch has some. */
const blocks = new Map();
function touch(b) {
  if (!blocks.has(b)) blocks.set(b, { t: 'blk', b, ts: 1756000000 + (b - GENESIS_BLOCK) * 3, d: [], e: [] });
  return blocks.get(b);
}
const span = LAST_BLOCK - GENESIS_BLOCK;
for (let i = 0; i < ACTIVE_BLOCKS; i++) {
  const b = GENESIS_BLOCK + Math.floor((i / ACTIVE_BLOCKS) * span) + (i % 7);
  const blk = touch(Math.min(b, LAST_BLOCK));
  const writes = 2 + ((rand() * 4) | 0);
  for (let w = 0; w < writes; w++) blk.d.push([felt(), felt()]);
  const events = 3 + ((rand() * 5) | 0);
  for (let e = 0; e < events; e++) blk.e.push([0, e, felt(), felt()]);
}

/** Mock note records — the anonymity set the trial scan walks. */
const noteBlocks = [...blocks.keys()].sort((x, y) => x - y);
for (let i = 0; i < MOCK_NOTES; i++) {
  const b = noteBlocks[Math.floor((i / MOCK_NOTES) * noteBlocks.length)];
  const blk = touch(b);
  const id = felt();
  (blk.n ??= []).push({
    id,
    tok: TOKEN_STRK,
    i: 0,
    amt: String(BigInt(1 + ((rand() * 500) | 0)) * 10n ** 15n),
    // Belongs to nobody: a tag under a key that exists nowhere.
    tag: tagFor(sha(felt()).slice(0, 64), id),
    from: felt(),
  });
}

// Our own note, and the spend of it, at the real block numbers.
{
  const blk = touch(NOTE_BLOCK);
  (blk.n ??= []).push({
    id: NOTE_ID,
    tok: TOKEN_STRK,
    i: 0,
    amt: '3000000000000000000',
    tag: tagFor(IDS.A.viewingKey, NOTE_ID),
    from: IDS.A.address,
  });
  blk.e.push([0, 0, 'EncNoteCreated', NOTE_ID]);
  const spend = touch(SPEND_BLOCK);
  (spend.x ??= []).push(nullifierFor(IDS.A.viewingKey, NOTE_ID));
  spend.e.push([0, 0, 'NoteUsed', NOTE_ID]);
}

const ordered = [...blocks.values()].sort((a, b) => a.b - b.b);

// ------------------------------------------------------------ cut the epochs

function epochOf(b) {
  return Math.floor(b / EPOCH_SIZE);
}

function cutEpoch(e, prevHash) {
  const from = e * EPOCH_SIZE;
  const to = from + EPOCH_SIZE - 1;
  const lines = [
    JSON.stringify({
      t: 'hdr',
      v: 1,
      kind: 'strk20-epoch',
      chain_id: CHAIN_ID,
      pool: POOL,
      epoch: e,
      from,
      to,
      prev: prevHash,
    }),
  ];
  for (const blk of ordered) if (epochOf(blk.b) === e) lines.push(JSON.stringify(blk));
  const payload = Buffer.from(lines.join('\n') + '\n', 'utf8');
  const z = zstdCompressSync(payload);
  return { entry: { e, from, to, hash: sha(payload), zst: sha(z), bytes: z.length, anchor: null }, z };
}

const epochs = [];
let prev = '0'.repeat(64);
for (let e = FIRST_EPOCH; e <= LAST_EPOCH; e++) {
  const cut = cutEpoch(e, prev);
  prev = cut.entry.hash;
  epochs.push(cut);
}

// ----------------------------------------------------------- write the stages

function headDoc(tailFrom, headBlock) {
  const lines = [
    JSON.stringify({
      t: 'hdr',
      v: 1,
      kind: 'strk20-head',
      tail_from: tailFrom,
      head: headBlock,
      head_hash: felt(),
      l1_accepted: headBlock - 200,
    }),
  ];
  for (const blk of ordered) if (blk.b >= tailFrom && blk.b <= headBlock) lines.push(JSON.stringify(blk));
  return Buffer.from(lines.join('\n') + '\n', 'utf8');
}

function writeStage(name, lastEpoch, tailFrom, headBlock, note) {
  const dir = join(OUT, name);
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(join(dir, 'epochs'), { recursive: true });

  const included = epochs.filter((c) => c.entry.e <= lastEpoch);
  for (const c of included) {
    writeFileSync(join(dir, 'epochs', `${String(c.entry.e).padStart(8, '0')}.strk20e.zst`), c.z);
  }
  writeFileSync(
    join(dir, 'genesis.json'),
    JSON.stringify(
      { format: 'strk20-feed', v: 1, chain_id: CHAIN_ID, pool: POOL, genesis_block: GENESIS_BLOCK, epoch_size: EPOCH_SIZE },
      null,
      2,
    ) + '\n',
  );
  const head = headDoc(tailFrom, headBlock);
  writeFileSync(join(dir, 'head.ndjson'), head);
  const manifest = {
    v: 1,
    chain_id: CHAIN_ID,
    pool: POOL,
    genesis_block: GENESIS_BLOCK,
    epoch_size: EPOCH_SIZE,
    head: { number: headBlock, hash: felt(), l1_accepted: headBlock - 200, class: felt(), decode_state: 'ok' },
    latest_epoch: lastEpoch,
    epochs: included.map((c) => c.entry),
  };
  const bytes = Buffer.from(JSON.stringify(manifest, null, 2) + '\n', 'utf8');
  writeFileSync(join(dir, 'manifest.json'), bytes);

  const zb = included.reduce((n, c) => n + c.z.length, 0);
  return {
    stage: name,
    note,
    epochs: included.length,
    requests: 1 + 1 + included.length + 1,
    feedBytes: zb + head.length + bytes.length,
    manifestSha256: sha(bytes),
  };
}

mkdirSync(OUT, { recursive: true });
const stages = [
  writeStage('t0', LAST_EPOCH - 2, 14330000, NOTE_BLOCK - 15, 'before the note'),
  writeStage('t1', LAST_EPOCH - 2, 14330000, NOTE_BLOCK + 85, 'the note is in the tail'),
  writeStage('t2', LAST_EPOCH, 14340000, LAST_BLOCK, 'the epoch was cut and the note was spent'),
];

const index = {
  generator: 'ts/demo/scripts/gen-replay-feed.mjs',
  synthetic: true,
  why: 'no captured Sepolia feed directory exists in this tree; see the header of the generator',
  chain_id: CHAIN_ID,
  pool: POOL,
  noteBlock: NOTE_BLOCK,
  spendBlock: SPEND_BLOCK,
  noteId: NOTE_ID,
  activeBlocks: blocks.size,
  events: ordered.reduce((n, b) => n + b.e.length, 0),
  mockNotes: MOCK_NOTES + 1,
  stages,
};
writeFileSync(join(OUT, 'index.json'), JSON.stringify(index, null, 2) + '\n');

for (const s of stages) {
  console.log(
    `${s.stage}: ${s.epochs} epochs, ${s.requests} requests, ${(s.feedBytes / 1024).toFixed(0)} kB, manifest ${s.manifestSha256.slice(0, 12)}…  (${s.note})`,
  );
}
console.log(`anonymity set: ${index.mockNotes} mock notes over ${index.activeBlocks} pool-active blocks`);
