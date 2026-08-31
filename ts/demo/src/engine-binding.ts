/**
 * ============================================================================
 *  THE ONE BINDING.
 * ============================================================================
 *
 * This file is the entire difference between the demo running on the mock and
 * the demo running on the real wasm module.
 *
 * The REAL engine is the default. The mock is reachable at `?engine=mock`, so
 * the demo still runs with no wasm build and no feed server — but it is never
 * reached by falling back. The wasm factory THROWS when the module is missing,
 * because a silent downgrade is exactly how a screenshot ends up claiming a
 * wasm number that a TypeScript stand-in produced. The UI renders
 * `ENGINE.label` and `ENGINE.provenance` in a badge that cannot be dismissed,
 * for the same reason.
 *
 * `loadGlue` is a callback rather than an import inside the package because
 * §4.10's chokepoint scan forbids a dynamic `import()` of a URL anywhere in
 * `strk20-discovery/src` except `net.ts`. The host does the import; the
 * package never does.
 */

import { mockEngineFactory } from 'strk20-discovery/engine/mock';
import { wasmEngineFactory, type WasmGlue } from 'strk20-discovery/engine/wasm';
import type { EngineFactory } from 'strk20-discovery';

const WASM: EngineFactory = wasmEngineFactory({
  loadGlue: () => import('./engine/strk20_engine.js') as unknown as Promise<WasmGlue>,
});

function requested(): 'wasm' | 'mock' {
  const p = new URLSearchParams(location.search).get('engine');
  return p === 'mock' ? 'mock' : 'wasm';
}

export const ENGINE: EngineFactory = requested() === 'mock' ? mockEngineFactory : WASM;
