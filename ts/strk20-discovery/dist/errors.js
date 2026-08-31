const RETRYABLE = new Set([
    'TRANSPORT',
    'FEED_ADVANCED_MIDSYNC',
    'SNAPSHOT_UNAVAILABLE',
    'SYNC_IN_PROGRESS',
]);
export class Strk20Error extends Error {
    code;
    details;
    retryable;
    constructor(code, message, details = {}) {
        super(message);
        this.name = 'Strk20Error';
        this.code = code;
        this.details = Object.freeze({ ...details });
        this.retryable = RETRYABLE.has(code);
    }
    /** The canonical JSON object the wasm module throws (§3.7), round-tripped. */
    static fromModuleJson(raw) {
        if (raw instanceof Strk20Error)
            return raw;
        const text = raw instanceof Error ? raw.message : String(raw);
        try {
            const o = JSON.parse(text);
            if (o && typeof o.code === 'string') {
                return new Strk20Error(o.code, o.message ?? o.code, o.details ?? {});
            }
        }
        catch {
            /* not canonical JSON — fall through */
        }
        return new Strk20Error('INTERNAL', text);
    }
    toJSON() {
        return { code: this.code, message: this.message, details: { ...this.details }, retryable: this.retryable };
    }
}
export function isStrk20Error(e) {
    return e instanceof Strk20Error;
}
//# sourceMappingURL=errors.js.map