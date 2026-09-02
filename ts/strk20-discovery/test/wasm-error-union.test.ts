/**
 * Dogfooding defect 3 — an exception escaping the adapter that was not a
 * `Strk20Error`.
 *
 * §4.2's error union is closed, and that is a promise about what comes OUT of
 * this package. An integrator writes
 *
 *     catch (e) { if (isStrk20Error(e)) show(e.code); else ??? }
 *
 * and there is no correct `else` to write — so a bare `TypeError` from the
 * planner does not merely look untidy, it lands in the branch the integrator
 * was told was unreachable. Defect 2 was one instance; this is the class.
 *
 * The two faults injected below are deliberately NOT the one defect 2 fixed:
 * fixing a single dereference is not the same as guaranteeing the boundary.
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { wasmEngineFactory } from '../src/engine-wasm.ts';
import { SEPOLIA } from '../src/profiles.ts';
import { isStrk20Error, Strk20Error } from '../src/errors.ts';
import type { StepFetch } from '../src/engine.ts';
import { COLD_INFO, stubGlue } from './stub-glue.ts';

const enc = new TextEncoder();
const env = (seq: number) => JSON.stringify({ seq, status: 200, not_modified: false, absent: false, etag: null });

const GENESIS = JSON.stringify({
  v: 1,
  chain_id: SEPOLIA.chainId,
  pool: SEPOLIA.pool,
  genesis_block: SEPOLIA.genesisBlock,
  epoch_size: SEPOLIA.epochSize,
});

async function engineOver(info?: Record<string, unknown> | string) {
  const stub = stubGlue(info === undefined ? {} : { info });
  const factory = wasmEngineFactory({ loadGlue: async () => stub.glue });
  const engine = await factory.create(JSON.stringify(SEPOLIA));
  const g = JSON.parse(engine.sync_begin('auto')) as StepFetch;
  engine.sync_supply(env(g.seq), enc.encode(GENESIS), null);
  return engine;
}

test('a planner fault over a malformed manifest surfaces as a Strk20Error', async () => {
  const engine = await engineOver();
  // `epochs: null` type-checks nowhere but arrives over the wire all the same.
  // The planner does `for (const ep of m.epochs)`, which throws TypeError.
  const manifest = JSON.stringify({
    v: 1,
    chain_id: SEPOLIA.chainId,
    pool: SEPOLIA.pool,
    genesis_block: SEPOLIA.genesisBlock,
    epoch_size: SEPOLIA.epochSize,
    head: { number: 1, hash: '0x1', l1_accepted: 1 },
    latest_epoch: null,
    epochs: null,
  });

  let thrown: unknown;
  try {
    engine.sync_supply(env(1), enc.encode(manifest), null);
  } catch (e) {
    thrown = e;
  }

  assert.ok(thrown !== undefined, 'the malformed manifest must not pass silently');
  assert.ok(!(thrown instanceof TypeError), 'a TypeError is not a member of the documented union');
  assert.ok(isStrk20Error(thrown), 'isStrk20Error() is the integrator’s only test, and it must answer true');
  const e = thrown as Strk20Error;
  assert.equal(e.code, 'INTERNAL');
  assert.equal(e.retryable, false);
  assert.match(e.message, /iterable|epochs|undefined|null/i, 'the original message survives, so this stays debuggable');
  assert.equal(e.details['thrown'], 'TypeError', 'and what it was before the union is recorded');
});

test('a module returning unparseable info() also surfaces as a Strk20Error', async () => {
  // A different fault on a different method: the guarantee is about the
  // boundary, not about one call site.
  const engine = await engineOver('not json at all');
  let thrown: unknown;
  try {
    engine.info();
  } catch (e) {
    thrown = e;
  }
  assert.ok(isStrk20Error(thrown), 'info() must not leak a SyntaxError either');
  assert.equal((thrown as Strk20Error).code, 'INTERNAL');
});

test('a real module error keeps its own code rather than being flattened', async () => {
  // The guard must convert what is not in the union, and pass through what is.
  // A boundary that answers INTERNAL for everything is not an improvement.
  const stub = stubGlue({ info: COLD_INFO });
  const engine = stub.glue.Engine as unknown as { prototype: { apply: () => string } };
  engine.prototype.apply = () => {
    throw new Error(JSON.stringify({ code: 'FEED_CHAIN_BROKEN', message: 'epoch 828 does not chain', details: {} }));
  };
  const factory = wasmEngineFactory({ loadGlue: async () => stub.glue });
  const e2e = await factory.create(JSON.stringify(SEPOLIA));
  const g = JSON.parse(e2e.sync_begin('auto')) as StepFetch;
  e2e.sync_supply(env(g.seq), enc.encode(GENESIS), null);
  const manifest = JSON.stringify({
    v: 1,
    chain_id: SEPOLIA.chainId,
    pool: SEPOLIA.pool,
    genesis_block: SEPOLIA.genesisBlock,
    epoch_size: SEPOLIA.epochSize,
    head: { number: 1, hash: '0x1', l1_accepted: 1 },
    latest_epoch: null,
    epochs: [],
  });
  const head = JSON.parse(e2e.sync_supply(env(1), enc.encode(manifest), null)) as StepFetch;

  assert.throws(
    () => e2e.sync_supply(env(head.seq), enc.encode('tail'), null),
    (e: unknown) => isStrk20Error(e) && (e as Strk20Error).code === 'FEED_CHAIN_BROKEN',
  );
});

test('a factory fault surfaces as a Strk20Error too', async () => {
  // `create` and `load` are as much of the boundary as the Engine methods are:
  // a caller catches around the whole thing.
  const stub = stubGlue();
  const factory = wasmEngineFactory({ loadGlue: async () => stub.glue });
  await assert.rejects(
    () => factory.create('{ not json'),
    (e: unknown) => isStrk20Error(e) && (e as Strk20Error).code === 'INTERNAL',
  );
  await assert.rejects(
    () => factory.load('{ not json', [new Uint8Array(1), new Uint8Array(1)]),
    (e: unknown) => isStrk20Error(e) && (e as Strk20Error).code === 'INTERNAL',
  );
});
