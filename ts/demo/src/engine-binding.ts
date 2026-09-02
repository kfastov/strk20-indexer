/**
 * ============================================================================
 *  THE ONE BINDING.
 * ============================================================================
 *
 * This file is the entire difference between the demo running on the mock and
 * the demo running on the real wasm module.
 *
 * The REAL engine runs every lane that has a real feed behind it. The one
 * exception is REPLAY, and it is a property of the FIXTURE, not a preference:
 * `scripts/gen-replay-feed.mjs` emits the mock engine's document shape (note
 * records under `n`, sha256 tags, four-element event tuples), which the real
 * codec rejects outright — `FEED_MALFORMED: bad event tuple`. So REPLAY asks
 * for the mock and LIVE/MAINNET ask for wasm, and `?engine=` overrides either
 * way. Making REPLAY real needs a captured Sepolia feed; until then the lane
 * carries the MOCK ENGINE badge and the `synthetic` chip.
 *
 * Nothing here ever FALLS BACK. The wasm factory THROWS when the module is
 * missing, because a silent downgrade is exactly how a screenshot ends up
 * claiming a wasm number that a TypeScript stand-in produced. The UI renders
 * the engine kind and its provenance in a badge that cannot be dismissed, for
 * the same reason.
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

/** `?engine=mock|wasm`. Absent means "whatever the lane needs". */
function requested(): 'wasm' | 'mock' | null {
  const p = new URLSearchParams(location.search).get('engine');
  return p === 'mock' ? 'mock' : p === 'wasm' ? 'wasm' : null;
}

const OVERRIDE = requested();

/** The engine for a lane, unless the URL demanded one. */
export function engineFor(laneWants: 'wasm' | 'mock'): EngineFactory {
  return (OVERRIDE ?? laneWants) === 'mock' ? mockEngineFactory : WASM;
}
