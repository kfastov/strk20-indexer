/**
 * Review finding 7: a poisoned artifact row never healed. §4.5's invalidation
 * table was honoured for the folded blob (`stateClear()` on a load failure) and
 * not for `artifacts`, so a row that failed `verifyServedHash` was left in
 * place and every later sync failed on it until somebody called `resetCache()`.
 * There was no per-row eviction to call — only `artifactClear()`, which throws
 * away a good cache to remove one bad entry.
 *
 * Scope, stated rather than implied: this covers the primitive the client now
 * calls, not the two call sites in `#syncOnce`. Reaching those needs a stub
 * feed server, a stub engine and a hand-built manifest, which is more machinery
 * than a MINOR fix earns.
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { MemoryStorage } from '../src/storage.ts';

const row = (n: number) => ({ hash: `${n}`.repeat(8), zbytes: Uint8Array.of(n) });

test('one artifact row can be evicted without clearing the cache', async () => {
  const s = new MemoryStorage();
  await s.open();
  await s.artifactPut('epoch:1', row(1));
  await s.artifactPut('epoch:2', row(2));

  await s.artifactDelete('epoch:1');

  assert.equal(await s.artifactGet('epoch:1'), null, 'the poisoned row is gone');
  assert.deepEqual((await s.artifactGet('epoch:2'))?.zbytes, Uint8Array.of(2), 'and the rest survives');
});

test('evicting a row that is not there is not an error', async () => {
  const s = new MemoryStorage();
  await s.open();
  await assert.doesNotReject(() => s.artifactDelete('epoch:404'));
});

test('a persisted row carries the hash it was admitted under', async () => {
  // The client now writes `hash: step.sha256` after `verifyServedHash` passed,
  // where it used to write `hash: ''` — including for prefetch bytes nothing
  // had checked. A row whose recorded hash is empty or wrong is rejected as a
  // cache miss on the next read, which is what makes the cache self-healing.
  const s = new MemoryStorage();
  await s.open();
  await s.artifactPut('epoch:7', { hash: 'abc', zbytes: Uint8Array.of(7) });
  assert.equal((await s.artifactGet('epoch:7'))?.hash, 'abc');
});
