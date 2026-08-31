/**
 * The in-page half of `capture-scan`.
 *
 * consumer-path.md §4.9 is explicit that the Rust scanner is NOT reimplemented
 * in TypeScript for the e2e capture; it is promoted to a bin and reused. What
 * IS needed in the page is the same *encoding list*, so demo-app.md §6.2's live
 * scan searches for the same 13 forms. §4.9 requires that list to live in ONE
 * shared fixture consumed by both scanners so the two cannot drift.
 *
 * `ENCODINGS_FIXTURE_V1` below is that fixture's TypeScript face. Leg d4 asserts
 * it byte-identical to the fixture the Rust scanner compiles against. Until the
 * Rust side is wired the assertion is pending, and `encodingsFixtureDigest()`
 * exists precisely so that comparison is one string equality rather than a
 * review of two lists.
 */
export type EncodingName = 'hex-minimal-lower' | 'hex-minimal-upper' | 'hex-padded-lower' | 'hex-padded-upper' | 'hex-0x-minimal-lower' | 'hex-0x-minimal-upper' | 'hex-0x-padded-lower' | 'hex-0x-padded-upper' | 'decimal' | 'base64' | 'base64url' | 'raw-bytes-be' | 'raw-bytes-le';
/** The one fixture. Order is part of the fixture. */
export declare const ENCODINGS_FIXTURE_V1: readonly EncodingName[];
export declare function encodingsFixtureDigest(): string;
/** Every encoding of one secret that the scanner will look for. */
export declare function encodeAll(secret: Uint8Array): {
    encoding: EncodingName;
    needle: string;
}[];
export interface ScanSurface {
    /** Human label for the row that hit, e.g. `url` / `header:accept` / `body`. */
    where: string;
    text: string;
}
export interface ScanHit {
    where: string;
    encoding: EncodingName;
    secretLabel: string;
    excerpt: string;
}
export interface ScanSecret {
    label: string;
    bytes: Uint8Array;
}
export declare function scan(surfaces: readonly ScanSurface[], secrets: readonly ScanSecret[]): ScanHit[];
/**
 * Flatten a RequestRecord-shaped thing into scannable surfaces. Deliberately
 * structural: the URL, every header name AND value, and the body. A scanner
 * that only looks at URLs proves much less than the claim we make.
 */
export declare function surfacesOfRequest(r: {
    url: string;
    method?: string;
    headers?: Readonly<Record<string, string>>;
    body?: string;
}): ScanSurface[];
//# sourceMappingURL=scan.d.ts.map