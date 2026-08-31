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
import type { RequestArtifact, RequestRecord } from './types.ts';
/**
 * §2.8.1's closed whole-path allowlist, plus `/live`. Anchored at both ends, so
 * a query string is not merely forbidden, it is unmatched.
 */
export declare const FEED_PATH_ALLOWLIST: readonly RegExp[];
export declare function isAllowedFeedPath(path: string): boolean;
export interface FetchSpec {
    base: string;
    /** Emitted by the module. The wrapper prefixes `base` and appends NOTHING. */
    path: string;
    artifact: RequestArtifact;
    purpose: 'feed' | 'live' | 'anchor-rpc';
    optional: boolean;
    ifNoneMatch?: string | null;
    signal?: AbortSignal | undefined;
}
export interface FetchOutcome {
    status: number;
    notModified: boolean;
    absent: boolean;
    etag: string | null;
    bytes: Uint8Array | null;
    record: RequestRecord;
}
/**
 * The type lives here too, so no other file needs to write `typeof fetch` — the
 * chokepoint scan is a text scan, and a package that has to carve exceptions
 * into it has already lost the property the scan protects.
 */
export type FetchLike = typeof globalThis.fetch;
export declare function resolveFetch(custom?: FetchLike): FetchLike;
export interface NetContext {
    fetchImpl: FetchLike;
    onRecord(r: RequestRecord): void;
    now(): number;
}
/** The one function. Every byte this package fetches goes through here. */
export declare function request(ctx: NetContext, spec: FetchSpec): Promise<FetchOutcome>;
/**
 * A record for bytes that came from IndexedDB rather than the wire. It is NOT a
 * network request and demo-app.md §9 forbids counting it as one; it is recorded
 * so the panel can show `network N · cache M` instead of a silent gap.
 */
export declare function cacheRecord(ctx: NetContext, spec: Pick<FetchSpec, 'base' | 'path' | 'artifact' | 'purpose'>, bytes: number, ms: number): RequestRecord;
export interface LiveStream {
    close(): void;
    readonly closed: boolean;
    readonly record: RequestRecord;
}
/**
 * `/feed/live` — parameterless, no auth, no cookies. The SSE connection is a
 * row in the panel like everything else (demo-app.md §6.2 rule 2), which is why
 * it returns its RequestRecord and keeps the byte counter live.
 */
export declare function openLive(ctx: NetContext, base: string, handlers: {
    onPoke(): void;
    onError(): void;
}): LiveStream;
/**
 * Delegated mode only (§4.8): the chain-identity probe that runs BEFORE any key
 * is sent. Here rather than in delegated.ts so the chokepoint holds.
 */
export declare function healthGet(ctx: NetContext, serverUrl: string): Promise<{
    chain_id?: string;
    pool?: string;
}>;
/**
 * Delegated mode only (§4.8). Separate function so the keyless path cannot
 * reach a code branch that attaches an Authorization header or a POST body.
 */
export declare function delegatedPost(ctx: NetContext, opts: {
    serverUrl: string;
    path: string;
    body: string;
    authToken?: string | undefined;
}): Promise<{
    status: number;
    text: string;
    record: RequestRecord;
}>;
//# sourceMappingURL=net.d.ts.map