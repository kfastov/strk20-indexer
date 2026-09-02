/**
 * Dogfooding defect 5 — `package.json` listed a README that did not exist, and
 * doc comments in `src/` referred to "the README paragraph".
 *
 * A README is only worth writing if its examples run, and the way examples rot
 * is that nothing compiles them. So the two the README shows are written out
 * below as real TypeScript against the real exports: `npm run typecheck`
 * covers `test/`, so a rename or a changed option shape breaks the build
 * rather than quietly leaving a lie in the docs.
 *
 * The runtime half checks the other direction: that every identifier the
 * README imports actually exists on the entry point it names, and that those
 * entry points are the ones `package.json` publishes.
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

import * as root from '../src/index.ts';
import * as delegatedEntry from '../src/delegated.ts';
import * as wasmEntry from '../src/engine-wasm.ts';
import * as mockEntry from '../src/engine-mock.ts';
import * as scanEntry from '../src/scan.ts';

import { KeylessClient, staticAccount, isStrk20Error } from '../src/index.ts';
import { DelegatedClient } from '../src/delegated.ts';
import { wasmEngineFactory, type WasmGlue } from '../src/engine-wasm.ts';

const HERE = dirname(fileURLToPath(import.meta.url));
const README = readFileSync(join(HERE, '..', 'README.md'), 'utf8');
const PKG = JSON.parse(readFileSync(join(HERE, '..', 'package.json'), 'utf8')) as {
  files: string[];
  exports: Record<string, string>;
};

// ------------------------------------------------------ the examples, compiled

/**
 * The README's KeylessClient example. Constructed, never run: it would fetch.
 *
 * The glue path is a `const` rather than the README's literal only because
 * `./pkg/strk20_engine.js` exists in a CONSUMER's tree, not in this one, and a
 * literal would fail module resolution here. The option shape, the factory and
 * every name below are the real ones.
 */
const GLUE_PATH = './pkg/strk20_engine.js';

function keylessExample(viewingKey32: Uint8Array) {
  const client = new KeylessClient({
    feedUrl: 'https://feed.example.org/sepolia',
    network: 'sepolia',
    engine: wasmEngineFactory({
      loadGlue: () => import(GLUE_PATH) as unknown as Promise<WasmGlue>,
    }),
  });
  return async () => client.getNotes(staticAccount('0x1234', viewingKey32));
}

/** The README's DelegatedClient example. */
function delegatedExample() {
  const delegated = new DelegatedClient({ serverUrl: 'https://sync.internal', network: 'sepolia' });
  return async () => delegated.verifyChainIdentity();
}

/** The README's error-handling example. */
async function errorExample(client: KeylessClient, report: (code: string, retryable: boolean) => void) {
  try {
    await client.sync();
  } catch (e) {
    if (isStrk20Error(e)) report(e.code, e.retryable);
    else throw e;
  }
}

test('the documented examples construct against the real exports', () => {
  // Compilation is the assertion `npm run typecheck` makes. This one proves the
  // constructors are reachable and do not throw on the documented options.
  assert.equal(typeof keylessExample(new Uint8Array(32)), 'function');
  assert.equal(typeof delegatedExample(), 'function');
  assert.equal(typeof errorExample, 'function');
});

// ------------------------------------------------------------ the README file

test('the README package.json promises actually exists', () => {
  assert.ok(PKG.files.includes('README.md'), 'package.json ships it');
  assert.ok(README.length > 0);
  // `trimEnd` so the single trailing newline every text file has is not counted
  // as an 81st line.
  assert.ok(README.trimEnd().split('\n').length <= 80, 'kept short enough that it is read rather than skimmed');
});

test('every entry point the README imports is one the package publishes', () => {
  const specifiers = [...README.matchAll(/from '(strk20-discovery[^']*)'/g)].map((m) => m[1]!);
  assert.ok(specifiers.length >= 3, 'the README shows imports at all');
  for (const s of new Set(specifiers)) {
    const subpath = s === 'strk20-discovery' ? '.' : `.${s.slice('strk20-discovery'.length)}`;
    assert.ok(PKG.exports[subpath], `${s} resolves to a published export (${subpath})`);
  }
});

test('every identifier the README imports really is exported', () => {
  const namespaces: Record<string, Record<string, unknown>> = {
    'strk20-discovery': root,
    'strk20-discovery/delegated': delegatedEntry,
    'strk20-discovery/engine/wasm': wasmEntry,
    'strk20-discovery/engine/mock': mockEntry,
    'strk20-discovery/scan': scanEntry,
  };

  const missing: string[] = [];
  for (const m of README.matchAll(/import \{([^}]*)\} from '(strk20-discovery[^']*)'/g)) {
    const ns = namespaces[m[2]!];
    assert.ok(ns, `${m[2]} is a known entry point`);
    for (const raw of m[1]!.split(',')) {
      const name = raw.trim();
      // `type X` is erased, so it has no runtime binding to look for. The
      // typecheck above is what covers those.
      if (!name || name.startsWith('type ')) continue;
      if (!(name in ns)) missing.push(`${name} from ${m[2]}`);
    }
  }
  assert.deepEqual(missing, [], 'the README must not import a name this package does not export');
});

test('the README states the TypeScript floor and the not-yet list', () => {
  assert.match(README, /5\.6/, 'the floor a consumer needs before they hit TS2315');
  for (const claim of ['discoverNotes', 'witness', 'export_reference_cursor', 'Not published to npm']) {
    assert.ok(README.includes(claim), `the honest list still mentions ${claim}`);
  }
});
