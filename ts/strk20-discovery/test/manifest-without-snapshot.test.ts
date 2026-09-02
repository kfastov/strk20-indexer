/**
 * Dogfooding defect 2 — a manifest with no `snapshot` key at all.
 *
 * The planner asked `m.snapshot !== null`, which is TRUE for a manifest that
 * simply omits the key (`undefined !== null`). It then planned a snapshot
 * fetch and dereferenced `m.snapshot!.file` two statements later, throwing a
 * bare `TypeError` out of the adapter. The very next line already had the
 * correct falsy check, which is what made this survive review.
 *
 * The `null` and omitted spellings both occur in feeds this repo has
 * published, so both are exercised here.
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { wasmEngineFactory } from '../src/engine-wasm.ts';
import { SEPOLIA } from '../src/profiles.ts';
import { Strk20Error } from '../src/errors.ts';
import type { Step, StepDone, StepFetch } from '../src/engine.ts';
import { stubGlue } from './stub-glue.ts';

const enc = new TextEncoder();
const env = (seq: number) => JSON.stringify({ seq, status: 200, not_modified: false, absent: false, etag: 'w/"1"' });

const GENESIS = JSON.stringify({
  format: 'strk20-feed',
  v: 1,
  chain_id: SEPOLIA.chainId,
  pool: SEPOLIA.pool,
  genesis_block: SEPOLIA.genesisBlock,
  epoch_size: SEPOLIA.epochSize,
});

/** `withSnapshot: 'omitted'` is the defect; `'null'` is the spelling that always worked. */
function manifest(snapshot: 'omitted' | 'null'): string {
  const m: Record<string, unknown> = {
    v: 1,
    chain_id: SEPOLIA.chainId,
    pool: SEPOLIA.pool,
    genesis_block: SEPOLIA.genesisBlock,
    epoch_size: SEPOLIA.epochSize,
    head: { number: 8280100, hash: '0x1', l1_accepted: 8280000 },
    latest_epoch: 827,
    epochs: [{ e: 827, from: 8270000, to: 8279999, hash: 'aa', zst: 'bb', bytes: 4 }],
  };
  if (snapshot === 'null') m['snapshot'] = null;
  return JSON.stringify(m);
}

/** Drive one full pass: genesis, manifest, whatever the plan then asks for. */
async function runToDone(snapshot: 'omitted' | 'null') {
  const stub = stubGlue();
  const factory = wasmEngineFactory({ loadGlue: async () => stub.glue });
  const engine = await factory.create(JSON.stringify(SEPOLIA));

  let step = JSON.parse(engine.sync_begin('auto')) as Step;
  const asked: string[] = [];
  // Bounded so a planning bug shows up as a failed assertion, not a hung suite.
  for (let i = 0; i < 20 && step.step === 'fetch'; i++) {
    const f = step as StepFetch;
    asked.push(f.artifact);
    const body =
      f.artifact === 'genesis' ? enc.encode(GENESIS) : f.artifact === 'manifest' ? enc.encode(manifest(snapshot)) : enc.encode('x');
    const payload = f.compressed ? enc.encode('inflated') : null;
    step = JSON.parse(engine.sync_supply(env(f.seq), body, payload)) as Step;
  }
  assert.equal(step.step, 'done', 'the pass must terminate in a done step');
  return { done: step as StepDone, asked, staged: stub.staged };
}

test('a manifest with no snapshot key loads and folds', async () => {
  const { done, asked, staged } = await runToDone('omitted');
  assert.deepEqual(asked, ['genesis', 'manifest', 'epoch', 'head'], 'no snapshot may be planned for a feed that has none');
  assert.deepEqual(staged, ['manifest', 'epoch:827', 'head'], 'and the fold saw exactly those artifacts');
  assert.equal(done.outcome.epochs_applied, 1);
  assert.equal(done.outcome.snapshot_basis, null);
});

test('and an explicit null snapshot behaves identically', async () => {
  const omitted = await runToDone('omitted');
  const explicit = await runToDone('null');
  assert.deepEqual(explicit.asked, omitted.asked, 'the two spellings of "no snapshot" are one case');
  assert.deepEqual(explicit.staged, omitted.staged);
});

test('coldStart:"snapshot" over a snapshotless feed is still a named refusal', async () => {
  // The falsy check must not have swallowed the case where the caller DEMANDED
  // a snapshot: that is a real refusal with a real code, not a silent fallback.
  const stub = stubGlue();
  const factory = wasmEngineFactory({ loadGlue: async () => stub.glue });
  const engine = await factory.create(JSON.stringify(SEPOLIA));
  const g = JSON.parse(engine.sync_begin('snapshot')) as StepFetch;
  engine.sync_supply(env(g.seq), enc.encode(GENESIS), null);
  assert.throws(
    () => engine.sync_supply(env(1), enc.encode(manifest('omitted')), null),
    (e: unknown) => e instanceof Strk20Error && e.code === 'SNAPSHOT_UNAVAILABLE',
  );
});
