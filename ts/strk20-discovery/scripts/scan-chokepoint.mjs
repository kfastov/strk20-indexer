#!/usr/bin/env node
/**
 * §4.10's mechanical enforcement.
 *
 * TypeScript has no type-system move that expresses "this module does no IO".
 * A scan over one filename is the checkable substitute, and it is what makes
 * `onRequest` honest rather than best-effort: if any file other than net.ts can
 * reach the network, the package's request record is a lie by omission exactly
 * where the audit matters.
 *
 * Run from CI beside leg u.
 */

import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = fileURLToPath(new URL('.', import.meta.url));
const SRC = join(here, '..', 'src');
const ALLOWED = new Set(['net.ts']);

const FORBIDDEN = [
  { re: /\bfetch\s*\(/, what: 'fetch(' },
  { re: /\btypeof\s+fetch\b/, what: 'typeof fetch' },
  { re: /\bXMLHttpRequest\b/, what: 'XMLHttpRequest' },
  { re: /\bEventSource\b/, what: 'EventSource' },
  { re: /\bsendBeacon\b/, what: 'sendBeacon' },
  { re: /\bWebSocket\b/, what: 'WebSocket' },
  { re: /\bimport\s*\(\s*[`'"]https?:/, what: 'dynamic import of a URL' },
  { re: /\bnavigator\.sendBeacon\b/, what: 'navigator.sendBeacon' },
];

function walk(dir) {
  const out = [];
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) out.push(...walk(p));
    else if (p.endsWith('.ts')) out.push(p);
  }
  return out;
}

const violations = [];
for (const file of walk(SRC)) {
  const rel = relative(SRC, file);
  if (ALLOWED.has(rel)) continue;
  const text = readFileSync(file, 'utf8');
  text.split('\n').forEach((line, i) => {
    // A line that is only a comment is documentation, not a call site.
    const code = line.replace(/^\s*(\/\/|\*|\/\*).*$/, '');
    for (const f of FORBIDDEN) {
      if (f.re.test(code)) violations.push(`${rel}:${i + 1}  ${f.what}  ${line.trim()}`);
    }
  });
}

if (violations.length > 0) {
  console.error('chokepoint violated — only src/net.ts may touch the network:\n');
  for (const v of violations) console.error('  ' + v);
  process.exit(1);
}
console.log(`chokepoint ok — the network is reachable only from src/net.ts`);
