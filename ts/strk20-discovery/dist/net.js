/**
 * src/net.ts — the ONLY place this package touches the network (§4.10).
 *
 * `scripts/scan-chokepoint.mjs` asserts mechanically that no other file under
 * `src/` contains `fetch`, `XMLHttpRequest`, `EventSource`, `sendBeacon` or a
 * dynamic `import()` of a URL. TypeScript has no type-system move that
 * expresses "this module does no IO"; a scan over one filename is the checkable
 * substitute, and it is what makes `onRequest` honest rather than best-effort.
 *
 * Obligations, from §4.10:
 *   1. emit a RequestRecord for every call;
 *   2. build the URL as `base + step.path` with NO interpolation of any
 *      caller-supplied string beyond the base;
 *   3. set no request header beyond Accept, If-None-Match (head only) and, in
 *      delegated mode, Authorization; credentials:'omit';
 *   4. reject at runtime any path outside the closed allowlist — whole-path
 *      match, never a prefix.
 */
import { Strk20Error } from "./errors.js";
/**
 * §2.8.1's closed whole-path allowlist, plus `/live`. Anchored at both ends, so
 * a query string is not merely forbidden, it is unmatched.
 */
export const FEED_PATH_ALLOWLIST = [
    /^\/genesis\.json$/,
    /^\/manifest\.json$/,
    /^\/epochs\/[0-9]{8}\.strk20e\.zst$/,
    /^\/epochs\/[0-9]{8}\.anchor\.json$/,
    /^\/snapshot\.strk20s\.zst$/,
    /^\/snapshot\.anchor\.json$/,
    /^\/anchors\.ndjson$/,
    /^\/head\.ndjson$/,
    /^\/live$/,
];
export function isAllowedFeedPath(path) {
    return FEED_PATH_ALLOWLIST.some((re) => re.test(path));
}
export function resolveFetch(custom) {
    const f = custom ?? globalThis.fetch?.bind(globalThis);
    if (!f)
        throw new Strk20Error('CONFIG_INVALID', 'no fetch implementation is available', { option: 'fetch' });
    return f;
}
function joinBase(base, path) {
    const b = base.endsWith('/') ? base.slice(0, -1) : base;
    return b + path;
}
function transferBytesFor(url) {
    // PerformanceResourceTiming.transferSize is 0 on cache hits and null
    // cross-origin without Timing-Allow-Origin. We report null rather than a
    // wrong 0 — demo-app.md §9 prints `n/a` for exactly this reason.
    const perf = globalThis.performance;
    if (!perf || typeof perf.getEntriesByName !== 'function')
        return null;
    const entries = perf.getEntriesByName(url, 'resource');
    const last = entries[entries.length - 1];
    if (!last)
        return null;
    return typeof last.transferSize === 'number' && last.transferSize > 0 ? last.transferSize : null;
}
/** The one function. Every byte this package fetches goes through here. */
export async function request(ctx, spec) {
    if (!isAllowedFeedPath(spec.path)) {
        throw new Strk20Error('SCOPE_VIOLATION', 'path is not in the closed feed allowlist', {
            path: spec.path,
        });
    }
    const url = joinBase(spec.base, spec.path);
    const headers = { Accept: '*/*' };
    if (spec.ifNoneMatch)
        headers['If-None-Match'] = spec.ifNoneMatch;
    const started = ctx.now();
    let status = 0;
    let bytes = null;
    let etag = null;
    let notModified = false;
    let absent = false;
    try {
        const res = await ctx.fetchImpl(url, {
            method: 'GET',
            headers,
            credentials: 'omit',
            redirect: 'error',
            ...(spec.signal ? { signal: spec.signal } : {}),
        });
        status = res.status;
        etag = res.headers.get('etag');
        if (status === 304) {
            notModified = true;
        }
        else if (status === 404 && spec.optional) {
            absent = true;
        }
        else if (status < 200 || status >= 300) {
            throw new Strk20Error('TRANSPORT', `feed responded ${status}`, { path: spec.path, status });
        }
        else {
            bytes = new Uint8Array(await res.arrayBuffer());
        }
    }
    catch (e) {
        if (e instanceof Strk20Error) {
            emit(ctx, spec, url, started, status, 0, 'network');
            throw e;
        }
        if (e?.name === 'AbortError') {
            throw new Strk20Error('ABORTED', 'sync aborted', { path: spec.path });
        }
        emit(ctx, spec, url, started, 0, 0, 'network');
        throw new Strk20Error('TRANSPORT', 'fetch failed', { path: spec.path });
    }
    const record = emit(ctx, spec, url, started, status, bytes?.length ?? 0, notModified ? 'etag-304' : 'network');
    return { status, notModified, absent, etag, bytes, record };
}
function emit(ctx, spec, url, started, status, bytes, source) {
    const record = {
        url,
        method: 'GET',
        purpose: spec.purpose,
        artifact: spec.artifact,
        status,
        bytes,
        transferBytes: transferBytesFor(url),
        requestBodyBytes: 0,
        source,
        ms: ctx.now() - started,
        at: started,
    };
    ctx.onRecord(record);
    return record;
}
/**
 * A record for bytes that came from IndexedDB rather than the wire. It is NOT a
 * network request and demo-app.md §9 forbids counting it as one; it is recorded
 * so the panel can show `network N · cache M` instead of a silent gap.
 */
export function cacheRecord(ctx, spec, bytes, ms) {
    const record = {
        url: joinBase(spec.base, spec.path),
        method: 'GET',
        purpose: spec.purpose,
        artifact: spec.artifact,
        status: 200,
        bytes,
        transferBytes: 0,
        requestBodyBytes: 0,
        source: 'idb-cache',
        ms,
        at: ctx.now() - ms,
    };
    ctx.onRecord(record);
    return record;
}
/**
 * `/feed/live` — parameterless, no auth, no cookies. The SSE connection is a
 * row in the panel like everything else (demo-app.md §6.2 rule 2), which is why
 * it returns its RequestRecord and keeps the byte counter live.
 */
export function openLive(ctx, base, handlers) {
    const url = joinBase(base, '/live');
    const record = {
        url,
        method: 'GET',
        purpose: 'live',
        artifact: 'live',
        status: 0,
        bytes: 0,
        transferBytes: null,
        requestBodyBytes: 0,
        source: 'network',
        ms: 0,
        at: ctx.now(),
    };
    ctx.onRecord(record);
    const ES = globalThis.EventSource;
    if (!ES) {
        record.status = 0;
        queueMicrotask(handlers.onError);
        return { close: () => { }, closed: true, record };
    }
    const es = new ES(url, { withCredentials: false });
    let closed = false;
    es.onopen = () => {
        record.status = 200;
    };
    es.onmessage = (ev) => {
        record.bytes += typeof ev.data === 'string' ? ev.data.length : 0;
        record.ms = ctx.now() - record.at;
        handlers.onPoke();
    };
    es.onerror = () => {
        record.ms = ctx.now() - record.at;
        if (!closed)
            handlers.onError();
    };
    return {
        close() {
            closed = true;
            es.close();
            record.ms = ctx.now() - record.at;
        },
        get closed() {
            return closed;
        },
        record,
    };
}
/**
 * Delegated mode only (§4.8): the chain-identity probe that runs BEFORE any key
 * is sent. Here rather than in delegated.ts so the chokepoint holds.
 */
export async function healthGet(ctx, serverUrl) {
    const url = serverUrl.replace(/\/$/, '') + '/health';
    const started = ctx.now();
    const res = await ctx.fetchImpl(url, { credentials: 'omit', redirect: 'error' });
    const text = await res.text();
    ctx.onRecord({
        url,
        method: 'GET',
        purpose: 'feed',
        artifact: 'rpc',
        status: res.status,
        bytes: text.length,
        transferBytes: null,
        requestBodyBytes: 0,
        source: 'network',
        ms: ctx.now() - started,
        at: started,
    });
    try {
        return JSON.parse(text);
    }
    catch {
        throw new Strk20Error('TRANSPORT', '/health did not return JSON', { status: res.status });
    }
}
/**
 * Delegated mode only (§4.8). Separate function so the keyless path cannot
 * reach a code branch that attaches an Authorization header or a POST body.
 */
export async function delegatedPost(ctx, opts) {
    const url = opts.serverUrl.replace(/\/$/, '') + opts.path;
    const headers = { Accept: 'application/json', 'Content-Type': 'application/json' };
    if (opts.authToken)
        headers['Authorization'] = `Bearer ${opts.authToken}`;
    const started = ctx.now();
    const res = await ctx.fetchImpl(url, {
        method: 'POST',
        headers,
        body: opts.body,
        credentials: 'omit',
        redirect: 'error',
    });
    const text = await res.text();
    const record = {
        url,
        method: 'POST',
        purpose: 'feed',
        artifact: 'rpc',
        status: res.status,
        bytes: text.length,
        transferBytes: null,
        requestBodyBytes: opts.body.length,
        source: 'network',
        ms: ctx.now() - started,
        at: started,
    };
    ctx.onRecord(record);
    return { status: res.status, text, record };
}
//# sourceMappingURL=net.js.map