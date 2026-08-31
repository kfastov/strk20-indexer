/**
 * The engine seam.
 *
 * `Engine` below is consumer-path.md §3.3's exported wasm ABI, transcribed
 * one-for-one. It is SYNCHRONOUS by contract: bytes in, notes out, no network,
 * no storage, no async inside the computer. Everything asynchronous — fetch,
 * IndexedDB, zstd, SSE — lives above this line, in TypeScript.
 *
 * Two implementations satisfy it:
 *   - `engine-wasm.ts` — the real one, calling the module built from
 *     `crates/strk20-engine`. Not available yet (the 0a refactor and the wasm
 *     build land later).
 *   - `engine-mock.ts` — a TypeScript stand-in that runs the SAME Step
 *     trampoline over a real static feed, with real fetches, real sha256
 *     verification and a real per-note trial scan.
 *
 * The demo switches between them by changing ONE binding
 * (`ts/demo/src/engine-binding.ts`) and nothing else.
 *
 * An `EngineFactory` carries a `kind`, a `label` and a `provenance` string, and
 * the client republishes all three through `status().engine`, so a screenshot
 * cannot misrepresent which computer produced a number.
 */
export {};
//# sourceMappingURL=engine.js.map