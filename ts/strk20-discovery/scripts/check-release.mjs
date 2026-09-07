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
  // Exercise the installed package against the same real-WASM fixture server,
  // including verified cold startup and an offline cached restart.
  cpSync(resolve(root, '../../crates/wasm/fixture'), join(stage, 'fixture'), { recursive: true });
  const nodeTest = readFileSync(join(root, 'test/node.test.ts'), 'utf8')
    .replace('../dist/node.js', 'strk20-discovery/node')
    .replace('../../../crates/wasm/fixture/', './fixture/');
  writeFileSync(join(stage, 'node.test.ts'), nodeTest);
  run(process.execPath, ['--test', '--experimental-strip-types', 'node.test.ts']);
  run(process.execPath, ['node_modules/typescript/bin/tsc', '--noEmit']);
  run(process.execPath, ['node_modules/vite/bin/vite.js', 'build']);
} finally { rmSync(stage, { recursive: true, force: true }); }
