/**
 * Dogfooding defect 4 — the published .d.ts required TypeScript 5.7.
 *
 * TypeScript 5.7 made `Uint8Array` generic. Two methods here had INFERRED
 * return types, so declaration emit wrote the inferred form —
 * `Promise<Uint8Array<ArrayBufferLike> | null>` and `zbytes:
 * Uint8Array<ArrayBuffer>` — into `dist/storage.d.ts`. A consumer on 5.6 then
 * failed with `TS2315: Type 'Uint8Array' is not generic` and could only get
 * past it with `skipLibCheck`, which silences every other typing problem too.
 *
 * The fix is an explicit annotation at each site: declaration emit reuses a
 * WRITTEN type node, and bare `Uint8Array` is valid on both sides of 5.7.
 * That is a property of the emitted artifact, not of the source, so this test
 * reads `dist/`.
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const DIST = join(dirname(fileURLToPath(import.meta.url)), '..', 'dist');

function typings(): { name: string; text: string }[] {
  let names: string[];
  try {
    names = readdirSync(DIST, { recursive: true }) as unknown as string[];
  } catch {
    // Not a skip: an unbuilt dist would let this guard report green while the
    // published typings were never looked at.
    assert.fail('dist/ is missing — run `npm run build` first; this test guards the PUBLISHED typings');
  }
  const out = names.filter((n) => String(n).endsWith('.d.ts')).map((n) => ({ name: String(n), text: readFileSync(join(DIST, String(n)), 'utf8') }));
  assert.ok(out.length > 0, 'dist/ holds no .d.ts files');
  return out;
}

test('no .d.ts uses a type argument on Uint8Array', () => {
  const offenders = typings()
    .flatMap(({ name, text }) =>
      text
        .split('\n')
        .map((line, i) => ({ name, line: line.trim(), n: i + 1 }))
        .filter((l) => /\bUint8Array\s*</.test(l.line)),
    )
    .map((l) => `${l.name}:${l.n}: ${l.line}`);

  assert.deepEqual(offenders, [], 'a generic Uint8Array in the emitted typings is TS2315 for a 5.6 consumer');
});

test('the typings still say Uint8Array where they should', () => {
  // The guard above is satisfied by deleting every mention. This is the other
  // half: the two methods the defect was reported against still carry the
  // annotation, spelled without a type argument.
  const storage = typings().find((t) => t.name.endsWith('storage.d.ts'));
  assert.ok(storage, 'storage.d.ts is part of the published surface');
  assert.match(storage.text, /cursorGet\(keyId: string\): Promise<Uint8Array \| null>/);
  assert.match(storage.text, /artifactGet\(key: string\): Promise<\{\s*hash: string;\s*zbytes: Uint8Array;\s*\} \| null>/);
});
