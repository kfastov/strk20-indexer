/**
 * The REAL engine adapter: `crates/wasm` (npm name `strk20-engine`), which is
 * `crates/consumer` compiled to `wasm32-unknown-unknown` behind a wasm-bindgen
 * facade. Same code as the native client — its smoke test folds a real feed
 * natively and through the module and demands the two `SyncReport`s be
 * byte-identical.
 *
 * ---------------------------------------------------------------------------
 * WHY THIS FILE IS NOT THIN, AND WHAT THAT MEANS
 * ---------------------------------------------------------------------------
 *
 * `engine.ts` transcribes consumer-path.md §3.3: a module that OWNS the fetch
 * plan and emits `Step::Fetch` for the wrapper to satisfy. The shipped module
 * does not have that ABI and deliberately never will — `crates/wasm/README.md`
 * §"Where §A3 turned out to be wrong" records why: `strk20-consumer` has no
 * byte-level apply, only `apply_feed(store, transport, cold_start)`, one
 * incremental pass that PULLS. So the module ships `stage_*` + `apply`
 * instead, and there is no `history()` and no `export_reference_cursor()`.
 *
 * This adapter is therefore the trampoline: it authors the fetch plan from the
 * manifest, drives it through the §3.3 Step JSON the client already knows how
 * to satisfy, and stages the returned bytes into the module.
 *
 * The honest consequence, stated because a demo that hides it is a demo that
 * lies: **TypeScript authors the URLs here, not Rust.** §4.10's claim that
 * "the component that authors the URLs cannot see a key" still holds — the
 * plan below is a pure function of `manifest.json` and the module's own
 * `info()`, and no branch of it can observe a viewing key, which is why the
 * A/B run still produces identical request logs for two identities. But the
 * claim is now enforced by this file rather than by the module's own
 * `request_log()`, which does not exist. `request_log_sha256()` below is
 * computed HERE and labelled as such in `provenance`.
 *
 * What Rust still owns, unchanged and un-reimplemented: the epoch hash chain,
 * the chain/pool binding, the §1.5 snapshot ladder, reachability grounding,
 * reorg supersede, the trial-decryption scan and the report. TypeScript never
 * verifies anything it then reports as verified.
 *
 * ---------------------------------------------------------------------------
 * KEY HANDLING
 * ---------------------------------------------------------------------------
 *
 * The whole discovery pass runs inside `discover_begin`, synchronously. That
 * is not laziness about the step API — it is the only shape in which the key
 * never outlives one synchronous call. The adapter copies the caller's bytes,
 * hands the copy to `Engine.discover`, which zeroizes it in place, and
 * zeroizes the caller's buffer too. Nothing between `begin` and `finish` holds
 * key material.
 */
import type { EngineFactory } from './engine.ts';
/** `crates/wasm/pkg/strk20_engine.d.ts`, exactly. */
interface WasmEngine {
    apply(coldStart: string): string;
    apply_head(payload: Uint8Array, etag: string): string;
    check_manifest(manifestJson: string): string;
    clear_storage_proofs(): void;
    discover(ownerHex: string, key: Uint8Array): string;
    export_state(): Uint8Array;
    forget_owner(ownerHex: string): void;
    info(): string;
    proof_candidates(): string;
    stage_anchors(payload: Uint8Array): void;
    stage_epoch(e: bigint, payload: Uint8Array): void;
    stage_head(payload: Uint8Array, etag: string): void;
    stage_manifest(manifestJson: string): void;
    stage_snapshot(e: bigint, zst: Uint8Array, payload: Uint8Array): void;
    stage_snapshot_anchor(e: bigint, json: Uint8Array): void;
    stage_storage_proof(block: bigint, proofJson: string, blockHashHex: string): void;
    free(): void;
}
interface WasmEngineClass {
    new (genesisJson: string): WasmEngine;
    load(blob: Uint8Array, genesisJson: string): WasmEngine;
    version(): string;
}
/** The wasm-pack `--target web` module namespace. */
export interface WasmGlue {
    default: (init?: unknown) => Promise<{
        memory: WebAssembly.Memory;
    }>;
    Engine: WasmEngineClass;
    set_panic_hook: () => void;
}
export interface WasmEngineOptions {
    /**
     * Supplies the wasm-pack glue. Injected rather than imported because §4.10's
     * chokepoint scan forbids a dynamic `import()` of a URL anywhere in `src/`
     * except `net.ts` — the host does the import, this package never does.
     */
    loadGlue: () => Promise<WasmGlue>;
    /**
     * Overrides the glue's default `new URL('strk20_engine_bg.wasm',
     * import.meta.url)`. Bytes are accepted too, which is how Node loads the
     * SAME artifact the browser does rather than a second build.
     */
    wasmUrl?: string | URL | BufferSource | WebAssembly.Module;
}
export declare function wasmEngineFactory(opts: WasmEngineOptions): EngineFactory;
export {};
//# sourceMappingURL=engine-wasm.d.ts.map