/**
 * Copies the built wasm module into the demo so Vite can bundle it.
 *
 * `crates/wasm/build.sh` writes `crates/wasm/pkg/` (wasm-pack `--target web`).
 * That directory is gitignored and lives outside the Vite root, so it is copied
 * to `src/engine/`, which is also gitignored. The `new URL('…_bg.wasm',
 * import.meta.url)` in the wasm-pack glue is a pattern Vite understands, so the
 * module ships as a hashed asset with no bundler configuration.
 *
 * Failing loudly is the point: a demo that quietly falls back to the mock is a
 * demo that puts a TypeScript stand-in's number under a "WASM ENGINE" badge.
 */
import { cpSync, existsSync, mkdirSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = fileURLToPath(new URL('.', import.meta.url));
const PKG = join(here, '..', '..', '..', 'crates', 'wasm', 'pkg');
const OUT = join(here, '..', 'src', 'engine');

if (!existsSync(join(PKG, 'strk20_engine_bg.wasm'))) {
  console.error(
    `\n  the wasm module is not built.\n  run:  ./crates/wasm/build.sh\n  expected: ${PKG}/strk20_engine_bg.wasm\n`,
  );
  process.exit(1);
}

mkdirSync(OUT, { recursive: true });
for (const f of ['strk20_engine.js', 'strk20_engine.d.ts', 'strk20_engine_bg.wasm', 'strk20_engine_bg.wasm.d.ts']) {
  cpSync(join(PKG, f), join(OUT, f));
}

const wasm = statSync(join(OUT, 'strk20_engine_bg.wasm')).size;
const glue = statSync(join(OUT, 'strk20_engine.js')).size;
console.log(`engine: ${(wasm / 1024).toFixed(0)} KB wasm + ${(glue / 1024).toFixed(0)} KB glue -> src/engine/`);
