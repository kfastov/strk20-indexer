/**
 * Two properties of the DEFAULT engine adapter, both of which were absent and
 * both of which are reachable from a `?feed=` URL parameter in the demo.
 *
 * Review finding 3 — the chain-identity pin was dropped on the wasm path. Both
 * factory entry points did `void profileJson`, so an empty mirror adopted
 * whatever chain the feed declared and then reported `replayed` for a genesis
 * the feed wrote. The mock engine had the check; the engine that ships did not.
 *
 * Review finding 4 — the trust grade was recomputed in TypeScript, where
 * `'anchored'` was unreachable by construction, and the demo displayed the
 * result labelled `provenance: 'measured'`.
 *
 * The glue is stubbed, not mocked-out: everything under test here is adapter
 * code, and stubbing the module is what lets a foreign genesis be pushed at it
 * without a feed server.
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { wasmEngineFactory, checkGenesisAgainstProfile, type WasmGlue } from '../src/engine-wasm.ts';
import { MAINNET, SEPOLIA } from '../src/profiles.ts';
import { Strk20Error } from '../src/errors.ts';
import type { StepFetch } from '../src/engine.ts';

const enc = new TextEncoder();

const genesisFor = (chainId: string, pool: string) =>
  JSON.stringify({ v: 1, chain_id: chainId, pool, genesis_block: MAINNET.genesisBlock, epoch_size: MAINNET.epochSize });

/** Records whether the module was ever constructed, which is the real question. */
function stubGlue(info: Record<string, unknown>): WasmGlue & { built: string[] } {
  const built: string[] = [];
  class StubEngine {
    constructor(genesisJson: string) {
      built.push(genesisJson);
    }
    info() {
      return JSON.stringify(info);
    }
    apply() {
      return JSON.stringify({
        epochs_applied: 0, last_epoch: null, last_epoch_to: 0, head: 0, l1_accepted: 0,
        tail_rewound: false, history_floor: 0, snapshot_basis: info.snapshot_basis ?? null,
        snapshot_rejected: false, state_changed: false,
      });
    }
    free() {}
  }
  const glue = {
    built,
    default: async () => ({ memory: new WebAssembly.Memory({ initial: 1 }) }),
    Engine: StubEngine as unknown as WasmGlue['Engine'],
    set_panic_hook: () => {},
  };
  return glue as unknown as WasmGlue & { built: string[] };
}

const RAW_INFO = {
  chain_id: MAINNET.chainId, pool: MAINNET.pool, genesis_block: MAINNET.genesisBlock,
  epoch_size: MAINNET.epochSize, last_epoch: 0, last_epoch_hash: 'aa', last_epoch_to: 10,
  history_floor: 0, snapshot_basis: 9, head: 10, l1_accepted: 10, slots: 3,
  tail_generation: 1, verified: 'server-asserted', engine_version: '0.0.0-test',
};

/** Drive the adapter to its first step and feed it a `genesis.json`. */
async function supplyGenesis(glue: WasmGlue, genesisJson: string) {
  const factory = wasmEngineFactory({ loadGlue: async () => glue });
  const engine = await factory.create(JSON.stringify(MAINNET));
  const step = JSON.parse(engine.sync_begin('auto')) as StepFetch;
  assert.equal(step.artifact, 'genesis', 'step 0 is always genesis');
  const env = { seq: step.seq, status: 200, not_modified: false, absent: false, etag: null };
  engine.sync_supply(JSON.stringify(env), enc.encode(genesisJson), null);
  return engine;
}

// ------------------------------------------------------- finding 3: the pin

test('a feed declaring a foreign chain is refused before the module is built', async () => {
  const glue = stubGlue(RAW_INFO);
  await assert.rejects(
    async () => supplyGenesis(glue, genesisFor(SEPOLIA.chainId, SEPOLIA.pool)),
    (e: unknown) => e instanceof Strk20Error && e.code === 'CHAIN_MISMATCH',
  );
  assert.deepEqual(glue.built, [], 'no engine may be constructed over a genesis that is not ours');
});

test('a feed declaring a foreign pool on the right chain is refused too', async () => {
  const glue = stubGlue(RAW_INFO);
  await assert.rejects(
    async () => supplyGenesis(glue, genesisFor(MAINNET.chainId, '0x' + '9'.repeat(63))),
    (e: unknown) => e instanceof Strk20Error && e.code === 'CHAIN_MISMATCH',
  );
  assert.deepEqual(glue.built, []);
});

test('geometry is pinned as well as identity', () => {
  const wrongEpochSize = JSON.stringify({
    chain_id: MAINNET.chainId, pool: MAINNET.pool,
    genesis_block: MAINNET.genesisBlock, epoch_size: MAINNET.epochSize + 1,
  });
  assert.throws(
    () => checkGenesisAgainstProfile(wrongEpochSize, MAINNET),
    (e: unknown) => e instanceof Strk20Error && e.code === 'CHAIN_MISMATCH',
  );
});

test('the pinned pool is compared as a felt, not as a string', () => {
  // A feed is under no obligation to spell an address the way the profile does.
  // Rejecting `0x0040…` against `0x40…` would reject an honest feed.
  const padded = MAINNET.pool.replace(/^0x/, '0x0000');
  assert.doesNotThrow(() => checkGenesisAgainstProfile(genesisFor(MAINNET.chainId, padded), MAINNET));
});

test('the matching genesis is accepted and the module is built from it', async () => {
  const glue = stubGlue(RAW_INFO);
  await supplyGenesis(glue, genesisFor(MAINNET.chainId, MAINNET.pool));
  assert.equal(glue.built.length, 1);
});

test('a persisted blob whose genesis is foreign is a cache miss, not a load', async () => {
  const glue = stubGlue(RAW_INFO);
  const factory = wasmEngineFactory({ loadGlue: async () => glue });
  const frames = [enc.encode(genesisFor(SEPOLIA.chainId, SEPOLIA.pool)), new Uint8Array([1, 2, 3])];
  assert.equal(await factory.load(JSON.stringify(MAINNET), frames), null);
  assert.deepEqual(glue.built, []);
});

// ----------------------------------------------------- finding 4: the grade

test('the grade comes from the module, including the one TypeScript could not express', async () => {
  // `snapshot_basis` is non-null, so the rule this adapter used to apply
  // locally would answer 'server-asserted' whatever the module said. The
  // module says 'anchored'.
  const glue = stubGlue({ ...RAW_INFO, verified: 'anchored' });
  const engine = await supplyGenesis(glue, genesisFor(MAINNET.chainId, MAINNET.pool));
  const info = JSON.parse(engine.info()) as { verified: string; snapshot_basis: number | null };
  assert.equal(info.snapshot_basis, 9, 'the discriminator: a local rule would say server-asserted here');
  assert.equal(info.verified, 'anchored');
});

test('and it is relayed unchanged when the module reports a weaker one', async () => {
  const glue = stubGlue({ ...RAW_INFO, snapshot_basis: null, verified: 'replayed' });
  const engine = await supplyGenesis(glue, genesisFor(MAINNET.chainId, MAINNET.pool));
  assert.equal((JSON.parse(engine.info()) as { verified: string }).verified, 'replayed');
});
