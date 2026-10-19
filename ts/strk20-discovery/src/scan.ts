/**
 * Browser-side generation of sensitive-value encodings for capture checks.
 * `ENCODINGS_FIXTURE_V1` describes the encoding set; compare its digest when
 * checking parity with another scanner. Keep this separate from feed transport.
 */

const toHex = (bytes: Uint8Array) =>
  Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");

export type EncodingName =
  | "hex-minimal-lower"
  | "hex-minimal-upper"
  | "hex-padded-lower"
  | "hex-padded-upper"
  | "hex-0x-minimal-lower"
  | "hex-0x-minimal-upper"
  | "hex-0x-padded-lower"
  | "hex-0x-padded-upper"
  | "decimal"
  | "base64"
  | "base64url"
  | "raw-bytes-be"
  | "raw-bytes-le";

/** The one fixture. Order is part of the fixture. */
export const ENCODINGS_FIXTURE_V1: readonly EncodingName[] = [
  "hex-minimal-lower",
  "hex-minimal-upper",
  "hex-padded-lower",
  "hex-padded-upper",
  "hex-0x-minimal-lower",
  "hex-0x-minimal-upper",
  "hex-0x-padded-lower",
  "hex-0x-padded-upper",
  "decimal",
  "base64",
  "base64url",
  "raw-bytes-be",
  "raw-bytes-le",
];

export function encodingsFixtureDigest(): string {
  return ENCODINGS_FIXTURE_V1.join("\n");
}

const B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/** Hand-rolled so the fixture behaves identically in a browser and in Node. */
function b64(bytes: Uint8Array): string {
  let out = "";
  for (let i = 0; i < bytes.length; i += 3) {
    const a = bytes[i]!;
    const b = bytes[i + 1];
    const c = bytes[i + 2];
    out += B64[a >> 2];
    out += B64[((a & 3) << 4) | ((b ?? 0) >> 4)];
    out += b === undefined ? "=" : B64[((b & 15) << 2) | ((c ?? 0) >> 6)];
    out += c === undefined ? "=" : B64[c & 63];
  }
  return out;
}

function stripLeadingZeros(hex: string): string {
  const s = hex.replace(/^0+/, "");
  return s.length === 0 ? "0" : s;
}

function latin1(bytes: Uint8Array): string {
  let s = "";
  for (const b of bytes) s += String.fromCharCode(b);
  return s;
}

/** Every encoding of one secret that the scanner will look for. */
export function encodeAll(
  secret: Uint8Array,
): { encoding: EncodingName; needle: string }[] {
  const padded = toHex(secret);
  const minimal = stripLeadingZeros(padded);
  const dec = (
    secret.length === 0 ? 0n : BigInt("0x" + (padded || "0"))
  ).toString(10);
  const rev = Uint8Array.from(secret).reverse();
  const out: { encoding: EncodingName; needle: string }[] = [
    { encoding: "hex-minimal-lower", needle: minimal },
    { encoding: "hex-minimal-upper", needle: minimal.toUpperCase() },
    { encoding: "hex-padded-lower", needle: padded },
    { encoding: "hex-padded-upper", needle: padded.toUpperCase() },
    { encoding: "hex-0x-minimal-lower", needle: "0x" + minimal },
    { encoding: "hex-0x-minimal-upper", needle: "0x" + minimal.toUpperCase() },
    { encoding: "hex-0x-padded-lower", needle: "0x" + padded },
    { encoding: "hex-0x-padded-upper", needle: "0x" + padded.toUpperCase() },
    { encoding: "decimal", needle: dec },
    { encoding: "base64", needle: b64(secret) },
    {
      encoding: "base64url",
      needle: b64(secret)
        .replace(/\+/g, "-")
        .replace(/\//g, "_")
        .replace(/=+$/, ""),
    },
    { encoding: "raw-bytes-be", needle: latin1(secret) },
    { encoding: "raw-bytes-le", needle: latin1(rev) },
  ];
  // A short needle would false-positive everywhere; the Rust scanner has the
  // same floor. Anything under 8 characters is not evidence.
  return out.filter((e) => e.needle.length >= 8);
}

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

export function scan(
  surfaces: readonly ScanSurface[],
  secrets: readonly ScanSecret[],
): ScanHit[] {
  const hits: ScanHit[] = [];
  const needles = secrets.flatMap((s) =>
    encodeAll(s.bytes).map((e) => ({ ...e, secretLabel: s.label })),
  );
  for (const surface of surfaces) {
    for (const n of needles) {
      const i = surface.text.indexOf(n.needle);
      if (i >= 0) {
        hits.push({
          where: surface.where,
          encoding: n.encoding,
          secretLabel: n.secretLabel,
          excerpt: surface.text.slice(
            Math.max(0, i - 12),
            i + n.needle.length + 12,
          ),
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
export function surfacesOfRequest(r: {
  url: string;
  method?: string;
  headers?: Readonly<Record<string, string>>;
  body?: string;
}): ScanSurface[] {
  const out: ScanSurface[] = [{ where: "url", text: r.url }];
  for (const [k, v] of Object.entries(r.headers ?? {})) {
    out.push({ where: `header-name:${k}`, text: k });
    out.push({ where: `header:${k}`, text: v });
  }
  if (r.body != null) out.push({ where: "body", text: r.body });
  return out;
}
