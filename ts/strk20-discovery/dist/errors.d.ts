/** §4.2's closed error union. Never widened to `string`. */
export type Strk20ErrorCode = 'FEED_HASH_MISMATCH' | 'FEED_CHAIN_BROKEN' | 'FEED_MALFORMED' | 'FEED_EPOCH_GAP' | 'FEED_ADVANCED_MIDSYNC' | 'DECOMPRESS_LIMIT' | 'DECOMPRESS_UNSTAGED' | 'SNAPSHOT_ROOT_MISMATCH' | 'SNAPSHOT_ANCHOR_MISSING' | 'SNAPSHOT_NOT_EMPTY' | 'SNAPSHOT_UNREACHABLE' | 'SNAPSHOT_UNAVAILABLE' | 'ANCHOR_UNBOUND' | 'BOUND_BELOW_SNAPSHOT' | 'CHAIN_MISMATCH' | 'STATE_CORRUPT' | 'STATE_VERSION' | 'STATE_FOREIGN' | 'SEALED_STATE_MISMATCH' | 'KEY_INVALID' | 'KEY_UNAVAILABLE' | 'ENTROPY_INVALID' | 'ENTROPY_REUSED' | 'DISCOVERY_INCOMPLETE' | 'HISTORY_UNAVAILABLE' | 'SYNC_PROTOCOL' | 'SYNC_IN_PROGRESS' | 'SCOPE_VIOLATION' | 'SESSION_INVALID' | 'SESSION_INCOMPLETE' | 'TRANSPORT' | 'CONFIG_INVALID' | 'ABORTED' | 'INTERNAL';
/**
 * `details` is a closed value type. It exists so a UI can say *which* option was
 * invalid without anybody being tempted to stuff a key-derived string into a
 * free-form field that a logger would then receive.
 */
export type ErrorDetail = Record<string, string | number | boolean | null>;
export declare class Strk20Error extends Error {
    readonly code: Strk20ErrorCode;
    readonly details: Readonly<ErrorDetail>;
    readonly retryable: boolean;
    constructor(code: Strk20ErrorCode, message: string, details?: ErrorDetail);
    /** The canonical JSON object the wasm module throws (§3.7), round-tripped. */
    static fromModuleJson(raw: unknown): Strk20Error;
    toJSON(): {
        code: Strk20ErrorCode;
        message: string;
        details: ErrorDetail;
        retryable: boolean;
    };
}
export declare function isStrk20Error(e: unknown): e is Strk20Error;
//# sourceMappingURL=errors.d.ts.map