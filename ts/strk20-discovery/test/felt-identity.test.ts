/**
 * Dogfooding defect 1 — the padded/unpadded pool address.
 *
 * `profiles.ts` writes Sepolia's pool zero-padded to 64 nibbles
 * (`0x0254a6b…`). Every feed this repo publishes writes it unpadded
 * (`0x254a6b…`; see `data/sepolia/idx/feed/genesis.json` and
 * `data/mainnet/feed/genesis.json`). Same felt, two spellings — and the mock
 * engine compared the STRINGS, so pointing it at the real published feed
 * produced `CHAIN_MISMATCH`, the loudest error this package has, for a
 * difference that carries no meaning. The wasm adapter already compared felts.
 *
 * The bytes below are the real ones, copied from the published feeds rather
 * than invented, because the invented ones are the ones that agreed.
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { MockEngine } from '../src/engine-mock.ts';
import { checkGenesisAgainstProfile } from '../src/engine-wasm.ts';
import { feltEq } from '../src/felt.ts';
import { MAINNET, SEPOLIA } from '../src/profiles.ts';
import { Strk20Error } from '../src/errors.ts';
import type { Step, StepFetch } from '../src/engine.ts';

const enc = new TextEncoder();

/** Verbatim from `data/sepolia/idx/feed/genesis.json`. Note the missing zero. */
const PUBLISHED_SEPOLIA_POOL = '0x254a6b2997ef52e9f830ce1f543f6b29768295e8d17e2267d672c552cfe0d91';
/** Verbatim from `data/mainnet/feed/genesis.json`. */
const PUBLISHED_MAINNET_POOL = '0x40337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a';

const genesisBytes = (pool: string) =>
  enc.encode(
    JSON.stringify({
      format: 'strk20-feed',
      v: 1,
      chain_id: SEPOLIA.chainId,
      pool,
      genesis_block: SEPOLIA.genesisBlock,
      epoch_size: SEPOLIA.epochSize,
    }),
  );

/** Drive a fresh mock to step 0 and hand it a `genesis.json`. */
function supplyGenesisToMock(bytes: Uint8Array): Step {
  const engine = new MockEngine(SEPOLIA);
  const step = JSON.parse(engine.sync_begin('auto')) as StepFetch;
  assert.equal(step.artifact, 'genesis');
  const env = { seq: step.seq, status: 200, not_modified: false, absent: false, etag: null };
  return JSON.parse(engine.sync_supply(JSON.stringify(env), bytes, null)) as Step;
}

test('the mock accepts the published feed genesis, whose pool is unpadded', () => {
  // This is the defect, exactly: SEPOLIA.pool has the leading zero and the
  // feed does not.
  assert.notEqual(PUBLISHED_SEPOLIA_POOL, SEPOLIA.pool, 'the spellings must differ or this test proves nothing');
  const next = supplyGenesisToMock(genesisBytes(PUBLISHED_SEPOLIA_POOL));
  assert.equal(next.step, 'fetch');
  assert.equal((next as StepFetch).artifact, 'manifest', 'genesis accepted; the run moves on to the manifest');
});

test('the wasm adapter accepts the same bytes', () => {
  assert.doesNotThrow(() =>
    checkGenesisAgainstProfile(new TextDecoder().decode(genesisBytes(PUBLISHED_SEPOLIA_POOL)), SEPOLIA),
  );
});

test('the mock still refuses a genuinely different pool', () => {
  // The fix must normalise spellings, never make two different felts equal.
  assert.throws(
    () => supplyGenesisToMock(genesisBytes(PUBLISHED_MAINNET_POOL)),
    (e: unknown) => e instanceof Strk20Error && e.code === 'CHAIN_MISMATCH',
  );
});

test('the manifest pool is compared the same way the genesis pool is', () => {
  // Fixing only `#onGenesis` would have moved the spurious rejection one step
  // later: `manifest.json` spells the pool exactly as `genesis.json` does.
  const engine = new MockEngine(SEPOLIA);
  const g = JSON.parse(engine.sync_begin('auto')) as StepFetch;
  const afterGenesis = JSON.parse(
    engine.sync_supply(
      JSON.stringify({ seq: g.seq, status: 200, not_modified: false, absent: false, etag: null }),
      genesisBytes(PUBLISHED_SEPOLIA_POOL),
      null,
    ),
  ) as StepFetch;
  assert.equal(afterGenesis.artifact, 'manifest');
  const manifest = enc.encode(
    JSON.stringify({
      v: 1,
      chain_id: SEPOLIA.chainId,
      pool: PUBLISHED_SEPOLIA_POOL,
      genesis_block: SEPOLIA.genesisBlock,
      epoch_size: SEPOLIA.epochSize,
      head: { number: SEPOLIA.genesisBlock, hash: '0x1', l1_accepted: SEPOLIA.genesisBlock },
      latest_epoch: 827,
      epochs: [{ e: 827, from: 8270000, to: 8279999, hash: 'aa', zst: 'bb', bytes: 1 }],
      snapshot: null,
    }),
  );
  const next = JSON.parse(
    engine.sync_supply(
      JSON.stringify({ seq: afterGenesis.seq, status: 200, not_modified: false, absent: false, etag: null }),
      manifest,
      null,
    ),
  ) as StepFetch;
  assert.equal(next.artifact, 'epoch', 'manifest accepted; the run proceeds to the one epoch it lists');
});

test('feltEq normalises spelling without merging distinct felts', () => {
  assert.ok(feltEq(PUBLISHED_SEPOLIA_POOL, SEPOLIA.pool));
  assert.ok(feltEq('0x0', '0x0000'));
  assert.ok(feltEq('0xABC', '0x0abc'), 'case is not part of a felt');
  assert.ok(!feltEq(PUBLISHED_MAINNET_POOL, SEPOLIA.pool));
  assert.ok(!feltEq('0x10', '0x1'), 'a trailing zero is not a leading one');
  assert.ok(!feltEq('0x1', 'not hex'));
  assert.ok(!feltEq(undefined, MAINNET.pool), 'an absent field is not a match');
});
