import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { resolve, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';

const root = fileURLToPath(new URL('../', import.meta.url));
const repo = resolve(root, '../..');
const sdk = resolve(repo, 'examples/mainnet/vendor/starknet-privacy-sdk');
const stage = mkdtempSync(join(tmpdir(), 'strk20-package-'));
const destination = resolve(process.env.PACK_DESTINATION ?? join(root, 'release'));
mkdirSync(destination, { recursive: true });
try {
  const pkg = JSON.parse(readFileSync(join(root, 'package.json')));
  const upstream = JSON.parse(readFileSync(join(sdk, 'package.json')));
  if (upstream.version !== '0.14.3-rc.5') throw new Error('Unexpected upstream SDK version');
  // Ship only the compiled upstream package. Public transitive dependencies
  // remain ordinary npm dependencies, not a copy of the development node_modules.
  pkg.dependencies = { ...upstream.dependencies, ...pkg.dependencies,
    [upstream.name]: upstream.version };
  pkg.bundleDependencies = [upstream.name];
  delete pkg.scripts;
  writeFileSync(join(stage, 'package.json'), JSON.stringify(pkg, null, 2) + '\n');
  for (const name of ['dist', 'README.md']) cpSync(join(root, name), join(stage, name), { recursive: true });
  cpSync(join(repo, 'LICENSE'), join(stage, 'LICENSE'));
  const included = join(stage, 'node_modules', upstream.name);
  mkdirSync(included, { recursive: true });
  cpSync(join(sdk, 'dist'), join(included, 'dist'), { recursive: true });
  cpSync(join(sdk, 'package.json'), join(included, 'package.json'));
  cpSync(join(repo, 'examples/mainnet/vendor/starknet-privacy/LICENSE'), join(included, 'LICENSE'));
  writeFileSync(join(stage, 'UPSTREAM.txt'),
    'Includes the unmodified compiled @starkware-libs/starknet-privacy-sdk 0.14.3-rc.5.\n' +
    'Source: https://github.com/starkware-libs/starknet-privacy/tree/PRIVACY-0.14.3-RC.5/sdk\n' +
    'Repository license is included alongside the upstream package; its package.json declares ISC.\n');
  pkg.files.push('LICENSE', 'UPSTREAM.txt');
  writeFileSync(join(stage, 'package.json'), JSON.stringify(pkg, null, 2) + '\n');
  process.stdout.write(execFileSync('npm', ['pack', '--json', '--ignore-scripts', '--pack-destination', destination], { cwd: stage }));
} finally { rmSync(stage, { recursive: true, force: true }); }
