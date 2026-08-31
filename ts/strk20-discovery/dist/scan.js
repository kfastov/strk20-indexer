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
import { toHex } from "./sha256.js";
/** The one fixture. Order is part of the fixture. */
export const ENCODINGS_FIXTURE_V1 = [
    'hex-minimal-lower',
    'hex-minimal-upper',
    'hex-padded-lower',
    'hex-padded-upper',
    'hex-0x-minimal-lower',
    'hex-0x-minimal-upper',
    'hex-0x-padded-lower',
    'hex-0x-padded-upper',
    'decimal',
    'base64',
    'base64url',
    'raw-bytes-be',
    'raw-bytes-le',
];
export function encodingsFixtureDigest() {
    return ENCODINGS_FIXTURE_V1.join('\n');
}
const B64 = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
/** Hand-rolled so the fixture behaves identically in a browser and in Node. */
function b64(bytes) {
    let out = '';
    for (let i = 0; i < bytes.length; i += 3) {
        const a = bytes[i];
        const b = bytes[i + 1];
        const c = bytes[i + 2];
        out += B64[a >> 2];
        out += B64[((a & 3) << 4) | ((b ?? 0) >> 4)];
        out += b === undefined ? '=' : B64[((b & 15) << 2) | ((c ?? 0) >> 6)];
        out += c === undefined ? '=' : B64[c & 63];
    }
    return out;
}
function stripLeadingZeros(hex) {
    const s = hex.replace(/^0+/, '');
    return s.length === 0 ? '0' : s;
}
function latin1(bytes) {
    let s = '';
    for (const b of bytes)
        s += String.fromCharCode(b);
    return s;
}
/** Every encoding of one secret that the scanner will look for. */
export function encodeAll(secret) {
    const padded = toHex(secret);
    const minimal = stripLeadingZeros(padded);
    const dec = (secret.length === 0 ? 0n : BigInt('0x' + (padded || '0'))).toString(10);
    const rev = Uint8Array.from(secret).reverse();
    const out = [
        { encoding: 'hex-minimal-lower', needle: minimal },
        { encoding: 'hex-minimal-upper', needle: minimal.toUpperCase() },
        { encoding: 'hex-padded-lower', needle: padded },
        { encoding: 'hex-padded-upper', needle: padded.toUpperCase() },
        { encoding: 'hex-0x-minimal-lower', needle: '0x' + minimal },
        { encoding: 'hex-0x-minimal-upper', needle: '0x' + minimal.toUpperCase() },
        { encoding: 'hex-0x-padded-lower', needle: '0x' + padded },
        { encoding: 'hex-0x-padded-upper', needle: '0x' + padded.toUpperCase() },
        { encoding: 'decimal', needle: dec },
        { encoding: 'base64', needle: b64(secret) },
        { encoding: 'base64url', needle: b64(secret).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '') },
        { encoding: 'raw-bytes-be', needle: latin1(secret) },
        { encoding: 'raw-bytes-le', needle: latin1(rev) },
    ];
    // A short needle would false-positive everywhere; the Rust scanner has the
    // same floor. Anything under 8 characters is not evidence.
    return out.filter((e) => e.needle.length >= 8);
}
export function scan(surfaces, secrets) {
    const hits = [];
    const needles = secrets.flatMap((s) => encodeAll(s.bytes).map((e) => ({ ...e, secretLabel: s.label })));
    for (const surface of surfaces) {
        for (const n of needles) {
            const i = surface.text.indexOf(n.needle);
            if (i >= 0) {
                hits.push({
                    where: surface.where,
                    encoding: n.encoding,
                    secretLabel: n.secretLabel,
                    excerpt: surface.text.slice(Math.max(0, i - 12), i + n.needle.length + 12),
                });
            }
        }
    }
    return hits;
}
/**
 * Flatten a RequestRecord-shaped thing into scannable surfaces. Deliberately
 * structural: the URL, every header name AND value, and the body. A scanner
 * that only looks at URLs proves much less than the claim we make.
 */
export function surfacesOfRequest(r) {
    const out = [{ where: 'url', text: r.url }];
    for (const [k, v] of Object.entries(r.headers ?? {})) {
        out.push({ where: `header-name:${k}`, text: k });
        out.push({ where: `header:${k}`, text: v });
    }
    if (r.body != null)
        out.push({ where: 'body', text: r.body });
    return out;
}
//# sourceMappingURL=scan.js.map