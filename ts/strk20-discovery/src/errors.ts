/** §4.2's closed error union. Never widened to `string`. */
export type Strk20ErrorCode =
  | 'FEED_HASH_MISMATCH'
  | 'FEED_CHAIN_BROKEN'
  | 'FEED_MALFORMED'
  | 'FEED_EPOCH_GAP'
  | 'FEED_ADVANCED_MIDSYNC'
  | 'DECOMPRESS_LIMIT'
  | 'DECOMPRESS_UNSTAGED'
  | 'SNAPSHOT_ROOT_MISMATCH'
  | 'SNAPSHOT_ANCHOR_MISSING'
  | 'SNAPSHOT_NOT_EMPTY'
  | 'SNAPSHOT_UNREACHABLE'
  | 'SNAPSHOT_UNAVAILABLE'
  | 'ANCHOR_UNBOUND'
  | 'BOUND_BELOW_SNAPSHOT'
  | 'CHAIN_MISMATCH'
  | 'STATE_CORRUPT'
  | 'STATE_VERSION'
  | 'STATE_FOREIGN'
  | 'SEALED_STATE_MISMATCH'
  | 'KEY_INVALID'
  | 'KEY_UNAVAILABLE'
  | 'ENTROPY_INVALID'
  | 'ENTROPY_REUSED'
  | 'DISCOVERY_INCOMPLETE'
  | 'HISTORY_UNAVAILABLE'
  | 'SYNC_PROTOCOL'
  | 'SYNC_IN_PROGRESS'
  | 'SCOPE_VIOLATION'
  | 'SESSION_INVALID'
  | 'SESSION_INCOMPLETE'
  | 'TRANSPORT'
  | 'CONFIG_INVALID'
  | 'ABORTED'
  | 'INTERNAL';

/**
 * `details` is a closed value type. It exists so a UI can say *which* option was
 * invalid without anybody being tempted to stuff a key-derived string into a
 * free-form field that a logger would then receive.
 */
export type ErrorDetail = Record<string, string | number | boolean | null>;

const RETRYABLE: ReadonlySet<Strk20ErrorCode> = new Set<Strk20ErrorCode>([
  'TRANSPORT',
  'FEED_ADVANCED_MIDSYNC',
  'SNAPSHOT_UNAVAILABLE',
  'SYNC_IN_PROGRESS',
]);

export class Strk20Error extends Error {
  readonly code: Strk20ErrorCode;
  readonly details: Readonly<ErrorDetail>;
  readonly retryable: boolean;

  constructor(code: Strk20ErrorCode, message: string, details: ErrorDetail = {}) {
    super(message);
    this.name = 'Strk20Error';
    this.code = code;
    this.details = Object.freeze({ ...details });
    this.retryable = RETRYABLE.has(code);
  }

  /** The canonical JSON object the wasm module throws (§3.7), round-tripped. */
  static fromModuleJson(raw: unknown): Strk20Error {
    if (raw instanceof Strk20Error) return raw;
    const text = raw instanceof Error ? raw.message : String(raw);
    try {
      const o = JSON.parse(text) as { code?: string; message?: string; details?: ErrorDetail };
      if (o && typeof o.code === 'string') {
        return new Strk20Error(o.code as Strk20ErrorCode, o.message ?? o.code, o.details ?? {});
      }
    } catch {
      /* not canonical JSON — fall through */
    }
    return new Strk20Error('INTERNAL', text);
  }

  toJSON(): { code: Strk20ErrorCode; message: string; details: ErrorDetail; retryable: boolean } {
    return { code: this.code, message: this.message, details: { ...this.details }, retryable: this.retryable };
  }
}

export function isStrk20Error(e: unknown): e is Strk20Error {
  return e instanceof Strk20Error;
}
