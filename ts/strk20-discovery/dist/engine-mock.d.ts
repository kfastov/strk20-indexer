/**
 * The MOCK engine.
 *
 * It exists because the real computer is not built to §3.3's second-pass ABI
 * yet (see engine-wasm.ts for the exact delta). Rather than block the demo —
 * whose entire purpose is to surface API problems early — the engine sits
 * behind `EngineFactory` and this implementation satisfies the same ABI in
 * TypeScript.
 *
 * It parses the REAL feed schema: `data/mainnet/feed` in this repo can be served
 * as a static directory and this engine folds it. That is deliberate. The
 * network layer, the trampoline, the verification ladder and the persistence
 * are then exercised against real bytes, and the only thing that changes when
 * the wasm module lands is which factory the demo binds.
 *
 * WHAT IS REAL HERE, because a mock that lies is worse than no demo:
 *   - the Step trampoline is the real protocol: the same asks, in the same
 *     order, with the same envelopes;
 *   - the fetches are real HTTP against a real static feed directory;
 *   - the sha256 of BOTH the compressed and the inflated buffer is verified
 *     against the manifest, and the epoch hash chain (`hdr.prev`) is walked. A
 *     tampered epoch really does raise FEED_HASH_MISMATCH;
 *   - the fold is a real parse of the real NDJSON and a real application of
 *     every storage diff into a slot map — on mainnet that is 139,131 writes
 *     over 134,879 distinct slots, which is the cost the browser actually pays;
 *   - the request log and its sha256 are computed HERE, inside the key-blind
 *     component, exactly as §3.3 requires — not by the UI.
 *
 * WHAT IS NOT REAL, stated so no number is misread:
 *   - it cannot decrypt real notes. Discovery is a trial scan over `n` records,
 *     a mock-only note encoding that the replay fixture carries and a real feed
 *     does not. Against a real feed discovery honestly finds nothing and says
 *     why (`mock_notes_available: false` in the report);
 *   - the tag derivation is sha256, not the pool's note encryption, so the
 *     per-note cost is far below the real trial decryption;
 *   - there is no MPT and no snapshot lane, so `verified` never rises above
 *     `replayed`.
 *
 * Consequence, republished through `status().engine` and rendered in the demo's
 * badge: **no timing from this engine is comparable to a native or wasm
 * measurement.** It is comparable to itself — which is exactly enough for the
 * cold-versus-warm contrast, the request and byte counts, and the
 * identical-stream claim, which are what the demo is for.
 */
import type { DiscoverOut, Engine, EngineFactory } from './engine.ts';
import type { ChainProfile } from './types.ts';
/** A mock-only note record. A real feed carries none; the replay fixture does. */
interface MockNote {
    id: string;
    tok: string;
    i: number;
    amt: string;
    tag: string;
    from: string;
}
interface NoteRow extends MockNote {
    b: number;
    t: number;
}
/**
 * Epoch-derived rows and tail-derived rows are separate compartments, because
 * §4.4 forbids persisting anything tail-derived: the "no persisted reorg logic"
 * property is enforced by the schema having nowhere to put a tail.
 */
interface Mirror {
    chain_id: string;
    pool: string;
    genesis_block: number;
    epoch_size: number;
    last_epoch: number;
    last_epoch_hash: string;
    last_epoch_to: number;
    history_floor: number;
    head: number;
    l1_accepted: number;
    slots: Record<string, string>;
    slotWrites: number;
    notes: NoteRow[];
    nullifiers: string[];
    blocks: number;
    events: number;
    mockNotesSeen: boolean;
    tailNotes: NoteRow[];
    tailNullifiers: string[];
}
export declare function noteTag(key: Uint8Array, noteId: string): string;
export declare function noteNullifier(key: Uint8Array, noteId: string): string;
export declare class MockEngine implements Engine {
    #private;
    constructor(profile: ChainProfile, mirror?: Mirror);
    /** Not part of the ABI. The demo pins it for the A/B comparison (§7 rule 2). */
    manifestHash(): string;
    info(): string;
    sync_begin(_coldStart: 'auto' | 'snapshot' | 'epochs'): string;
    sync_supply(metaJson: string, compressed: Uint8Array | null, payload: Uint8Array | null): string;
    sync_supply_rpc(_metaJson: string, _resultJson: string | null): string;
    sync_abort(): void;
    request_log(): string;
    request_log_sha256(): string;
    export_begin(): number;
    export_chunk(i: number): Uint8Array;
    export_end(): void;
    static loadFrames(profile: ChainProfile, frames: readonly Uint8Array[]): MockEngine | null;
    discover_begin(ownerHex: string, key: Uint8Array, sealed: Uint8Array | null, entropy32: Uint8Array): number;
    discover_step(handle: number, maxOps: number): string;
    discover_finish(handle: number): DiscoverOut;
    discover_abort(handle: number): void;
    history(ownerHex: string, key: Uint8Array, _sealed: Uint8Array | null, fromBlock: number | null, limit: number): string;
    export_reference_cursor(key: Uint8Array, sealed: Uint8Array): string;
    free(): void;
    memoryBytes(): number;
}
export declare function epochPath(e: number): string;
export declare const mockEngineFactory: EngineFactory;
export {};
//# sourceMappingURL=engine-mock.d.ts.map