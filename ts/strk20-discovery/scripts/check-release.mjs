import { cpSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';

const root = fileURLToPath(new URL('../', import.meta.url));
const pkg = JSON.parse(readFileSync(join(root, 'package.json')));
const stage = mkdtempSync(join(tmpdir(), 'strk20-consumer-'));
const run = (command, args) => execFileSync(command, args, { cwd: stage, stdio: 'inherit' });
try {
  writeFileSync(join(stage, 'package.json'), JSON.stringify({ private: true, type: 'module' }));
  run('npm', ['install', '--ignore-scripts', '--no-audit', '--no-fund',
    join(root, 'release', `${pkg.name}-${pkg.version}.tgz`),
    'vite@7.1.5', 'typescript@5.9.2', '@types/node@24']);
  for (const name of ['src', 'index.html', 'tsconfig.json', 'vite.config.ts'])
    cpSync(resolve(root, '../demo', name), join(stage, name), { recursive: true });
  writeFileSync(join(stage, 'check.mjs'), `
    import assert from 'node:assert/strict';
    import {NodeDiscoveryProvider} from 'strk20-discovery/node';
    import {createPrivateTransfers, Witness} from 'strk20-discovery/privacy-sdk';
    const provider = new NodeDiscoveryProvider({network:'sepolia', feedUrl:'https://unused.invalid', cacheDirectory:'./cache'});
    try {
      assert.equal(typeof createPrivateTransfers, 'function');
      assert.equal(typeof Witness, 'function');
      assert.equal((await provider.ready).verifiedAt, null);
    } finally { await provider.close(); }
  `);
  run(process.execPath, ['check.mjs']);
  run(process.execPath, ['node_modules/typescript/bin/tsc', '--noEmit']);
  run(process.execPath, ['node_modules/vite/bin/vite.js', 'build']);
} finally { rmSync(stage, { recursive: true, force: true }); }
