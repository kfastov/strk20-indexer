/**
 * Rendering. Pure functions from state to DOM, no measurement taken here.
 *
 * The two rules this file exists to keep:
 *   1. no number is displayed that was not produced in this session. A column
 *      that has not been run reads `not run yet` — never `0`, never a
 *      last-known value. If a measurement cannot be taken the slot reads
 *      `unavailable` AND WHY.
 *   2. the recorded reference numbers are grey, dated, sourced, and outside
 *      every live readout.
 */

import type { RequestRecord } from 'strk20-discovery';
import { REFERENCE, REFERENCE_SOURCE, DERIVED } from './reference-numbers.generated.ts';
import { shortAddress } from './identities.ts';
import type { DemoState, LogLine, RunCard } from './state.ts';

export function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  attrs: Record<string, string> = {},
  ...children: (Node | string | null | false)[]
): HTMLElementTagNameMap[K] {
  const n = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === 'class') n.className = v;
    else n.setAttribute(k, v);
  }
  for (const c of children) {
    if (c === null || c === false) continue;
    n.append(typeof c === 'string' ? document.createTextNode(c) : c);
  }
  return n;
}

export function ms(v: number | null | undefined): string {
  if (v === null || v === undefined) return '—';
  if (v < 1000) return `${v.toFixed(v < 10 ? 1 : 0)} ms`;
  return `${(v / 1000).toFixed(2)} s`;
}

export function bytes(v: number | null | undefined): string {
  if (v === null || v === undefined) return '—';
  if (v < 1024) return `${v} B`;
  if (v < 1024 * 1024) return `${(v / 1024).toFixed(1)} kB`;
  return `${(v / 1024 / 1024).toFixed(2)} MB`;
}

function num(v: number | null | undefined): string {
  return v === null || v === undefined ? '—' : v.toLocaleString('en-US');
}

// ------------------------------------------------------------------- cards

function cardRow(label: string, value: string, opts: { struck?: boolean; note?: string } = {}): HTMLElement {
  return el(
    'div',
    { class: `row${opts.struck ? ' struck' : ''}` },
    el('span', { class: 'row-label' }, label),
    el('span', { class: 'row-value' }, value),
    opts.note ? el('span', { class: 'row-note' }, opts.note) : null,
  );
}

export function renderCard(c: RunCard, title: string, subtitle: string): HTMLElement {
  const body: HTMLElement[] = [];
  // The reload run is timed from performance.timeOrigin, so engine boot and the
  // IndexedDB open are INSIDE its total. Every other run times sync() alone.
  const bootInTotal = c.kind === 'warm-reload';

  if (c.unavailable) {
    body.push(el('div', { class: 'unavailable' }, `unavailable — ${c.unavailable}`));
  } else if (c.totalMs === null) {
    body.push(el('div', { class: 'not-run' }, 'not run yet'));
  } else {
    body.push(cardRow('total', ms(c.totalMs)));
    // Both columns carry the SAME phase rows, so the reader watches the
    // subtraction happen. The warm column still fetches genesis, manifest and
    // head, so its fetch row is a real measurement; the struck row is the one
    // that actually vanished.
    body.push(cardRow(' fetch', ms(c.fetchMs)));
    body.push(cardRow(' inflate', ms(c.inflateMs)));
    body.push(cardRow(' verify+fold', ms(c.applyMs)));
    if (c.kind !== 'cold' && c.epochs === 0 && c.bytesSaved !== null && c.bytesSaved > 0) {
      body.push(
        cardRow(' epochs refetched', 'none', { struck: true, note: `${bytes(c.bytesSaved)} saved` }),
      );
    }
    body.push(cardRow(' load', c.loadMs === null || c.loadMs === 0 ? '—' : ms(c.loadMs)));
    body.push(cardRow(' discover', ms(c.discoverMs)));
    body.push(
      cardRow(
        'requests',
        `${num(c.networkRequests)} network${c.cacheRequests ? ` · ${num(c.cacheRequests)} cache` : ''}`,
      ),
    );
    body.push(cardRow('bytes', bytes(c.bytes)));
    body.push(cardRow('epochs fetched', num(c.epochs)));
    body.push(cardRow('epochs folded', num(c.epochsApplied)));
    body.push(
      cardRow('engine boot', c.bootMs === null ? 'not measured' : ms(c.bootMs), {
        note: bootInTotal ? 'in total' : 'not in total',
      }),
    );
  }

  return el(
    'section',
    { class: `card card-${c.kind}` },
    el('h3', {}, title, el('small', {}, bootInTotal ? 'after reload' : subtitle)),
    el('div', { class: 'card-body' }, ...body),
    c.ranAt
      ? el('div', { class: 'card-stamp' }, `${c.ranAt} · ${c.lane ?? ''} · ${c.feedUrl ?? ''}`)
      : el('div', { class: 'card-stamp' }, ''),
  );
}

// --------------------------------------------------------------------- log

export function renderLog(s: DemoState): HTMLElement {
  const rows = s.log.map((l) => renderLogLine(l));
  return el('div', { class: 'log' }, ...rows);
}

function renderLogLine(l: LogLine): HTMLElement {
  const time = new Date().toLocaleTimeString('en-GB', { hour12: false });
  const right =
    l.status === 'pending'
      ? el('span', { class: 'log-elapsed pending', 'data-since': String(l.at) }, '')
      : el('span', { class: 'log-elapsed' }, l.elapsedMs === undefined ? '' : ms(l.elapsedMs));
  const metrics = l.metrics?.length
    ? el(
        'div',
        { class: 'log-metrics' },
        ...l.metrics.map((m) =>
          el('span', { class: `metric prov-${m.provenance}` }, `${m.label} ${m.value}`),
        ),
      )
    : null;
  return el(
    'div',
    { class: `log-line status-${l.status}`, 'data-seq': String(l.seq) },
    el('span', { class: 'log-time' }, l.status === 'pending' ? '' : `[${time}]`),
    el('span', { class: 'log-stage' }, l.stage),
    el('span', { class: 'log-text' }, l.status === 'pending' ? `▸ ${l.text}` : l.text),
    el('span', { class: 'log-dots' }, ''),
    right,
    metrics,
    l.detail ? el('div', { class: 'log-detail' }, l.detail) : null,
  );
}

// ----------------------------------------------------------- network panel

const GROUP_AFTER = 50;

export function renderNetwork(s: DemoState, onSelfTest: () => void): HTMLElement {
  const rows: HTMLElement[] = [];
  const epochRows = s.records.filter((r) => r.artifact === 'epoch');
  const grouped = epochRows.length > GROUP_AFTER;

  let epochGroupInserted = false;
  for (const r of s.records) {
    if (grouped && r.artifact === 'epoch') {
      if (!epochGroupInserted) {
        epochGroupInserted = true;
        rows.push(renderEpochGroup(epochRows));
      }
      continue;
    }
    rows.push(renderRequestRow(r));
  }

  const network = s.records.filter((r) => r.source !== 'idb-cache');
  const cache = s.records.length - network.length;
  const total = s.records.reduce((n, r) => n + r.bytes, 0);

  return el(
    'section',
    { class: 'card card-net' },
    el('h3', {}, 'C · network'),
    el('code', { class: 'csp' }, readCsp()),
    el('div', { class: 'net-rows' }, ...(rows.length ? rows : [el('div', { class: 'not-run' }, 'no requests yet')])),
    el(
      'div',
      { class: 'net-totals' },
      el('div', {}, `${network.length} network · ${cache} from IndexedDB · ${bytes(total)}`),
      el(
        'div',
        { class: 'module-hash' },
        'request-log sha256 ',
        el('code', {}, s.feed?.network.requestLogSha256.slice(0, 16) || '—'),
      ),
    ),
    renderAb(s),
    renderScanner(s, onSelfTest),
    renderArithmetic(s),
  );
}

function renderEpochGroup(epochRows: RequestRecord[]): HTMLElement {
  const b = epochRows.reduce((n, r) => n + r.bytes, 0);
  const first = epochRows[0]?.url.split('/').pop() ?? '';
  const last = epochRows[epochRows.length - 1]?.url.split('/').pop() ?? '';
  const details = el('details', { class: 'net-group' });
  details.append(el('summary', {}, `epochs ${first} – ${last} · ${epochRows.length} requests · ${bytes(b)}`));
  for (const r of epochRows) details.append(renderRequestRow(r));
  return details;
}

function renderRequestRow(r: RequestRecord): HTMLElement {
  const hasQuery = r.url.includes('?');
  return el(
    'div',
    { class: `net-row src-${r.source} purpose-${r.purpose}` },
    el('span', { class: 'nr-method' }, r.method),
    // The full URL, never truncated in the middle: truncation is exactly where
    // a query string would hide.
    el('span', { class: 'nr-url' }, r.url),
    el('span', { class: `nr-query ${hasQuery ? 'bad' : 'good'}` }, hasQuery ? '?present' : 'no ?'),
    el('span', { class: 'nr-body' }, `body ${r.requestBodyBytes} B`),
    el('span', { class: 'nr-status' }, r.status === 0 ? 'open' : String(r.status)),
    el('span', { class: 'nr-bytes' }, bytes(r.bytes)),
    el('span', { class: 'nr-ms' }, ms(r.ms)),
    el('span', { class: 'nr-src' }, r.source),
    el(
      'span',
      { class: 'nr-transfer' },
      r.transferBytes === null ? 'transfer n/a' : `transfer ${bytes(r.transferBytes)}`,
    ),
  );
}

function renderAb(s: DemoState): HTMLElement {
  if (!s.ab) return el('div', { class: 'ab not-run' }, 'A/B: not run yet');
  const a = s.ab;
  const verdict =
    a.status === 'identical'
      ? el('span', { class: 'ok' }, '✓ IDENTICAL')
      : a.status === 'different'
        ? el('span', { class: 'fail' }, '✗ DIFFERENT')
        : el('span', { class: 'warn' }, 'no verdict');
  return el(
    'div',
    { class: `ab ab-${a.status}` },
    el('div', {}, 'A/B request-log hash ', verdict),
    el('div', { class: 'ab-hash' }, `A ${a.hashA.slice(0, 16)}…`),
    el('div', { class: 'ab-hash' }, `B ${a.hashB.slice(0, 16)}…`),
    el(
      'div',
      { class: 'ab-bytes' },
      `bytes ${bytes(a.bytesA)} / ${bytes(a.bytesB)} · requests ${a.requestsA} / ${a.requestsB}`,
    ),
  );
}

function renderScanner(s: DemoState, onSelfTest: () => void): HTMLElement {
  const btn = el('button', { class: 'small' }, 'self-test');
  if (s.identity) btn.addEventListener('click', onSelfTest);
  else {
    btn.setAttribute('disabled', 'disabled');
    btn.setAttribute('title', 'needs an identity');
  }
  return el(
    'div',
    { class: 'scanner' },
    el(
      'div',
      { class: s.scanHits === 0 ? 'ok' : 'fail' },
      `key + address: ${s.scanHits} hits · ${s.scanSurfaces} surfaces · ${s.records.length} requests · 13 encodings`,
    ),
    el(
      'div',
      { class: 'selftest' },
      btn,
      el(
        'span',
        { class: s.selfTestFired === null ? 'dim' : s.selfTestFired ? 'ok' : 'fail' },
        s.selfTestFired === null ? ' not run' : s.selfTestFired ? ' caught ✓' : ' MISSED ✗',
      ),
    ),
  );
}

function renderArithmetic(s: DemoState): HTMLElement {
  return el(
    'details',
    { class: 'arithmetic' },
    el('summary', {}, 'arithmetic · recorded numbers'),
    el(
      'pre',
      {},
      `this session:   1 genesis + 1 manifest + N epochs + 1 head\n` +
        `mainnet:        ${DERIVED.mainnetRequests!.formula} = ${DERIVED.mainnetRequests!.value}   [arithmetic]\n` +
        `with snapshots: 1 + 1 + 1 snapshot + 1 anchor + (0–1 epochs) + 1 head ≈ 5   [arithmetic]\n` +
        `this run:       ${s.records.filter((r) => r.source !== 'idb-cache').length} network requests   [measured]`,
    ),
    el(
      'table',
      { class: 'refs' },
      ...Object.values(REFERENCE).map((r) =>
        el(
          'tr',
          {},
          el('td', {}, r.name),
          el('td', {}, r.value),
          el('td', {}, `${REFERENCE_SOURCE}:${r.sourceLine}`),
        ),
      ),
    ),
  );
}

function readCsp(): string {
  const meta = document.querySelector('meta[http-equiv="Content-Security-Policy"]');
  return meta?.getAttribute('content') ?? '(no CSP meta tag on this page)';
}

// -------------------------------------------------------------- trust card

export function renderTrust(s: DemoState): HTMLElement {
  const f = s.feed;
  const grade = f?.verified ?? null;
  const gradeEl = el(
    'span',
    { class: `grade grade-${grade ?? 'none'}` },
    grade ?? 'not run yet',
  );
  return el(
    'section',
    { class: 'card card-trust' },
    el('h3', {}, 'B · trust'),
    el(
      'div',
      { class: 'trust-line' },
      'verified = ',
      gradeEl,
      f
        ? ` · head ${num(f.head)} · l1 ${num(f.l1Accepted)} · lastEpoch ${f.lastEpoch} · floor ${num(f.historyFrom)} · staleness ${f.staleness}`
        : '',
    ),
    el(
      'div',
      { class: 'persistence' },
      `persistence: ${s.persistence} · persisted() = ${s.persisted}`,
    ),
  );
}

// ------------------------------------------------------------------ badges

/**
 * Derived from ENGINE.kind, so a mock run cannot be screenshotted without it.
 * The full provenance is the title attribute — available on hover, never prose
 * on the page.
 */
export function renderEngineBadge(s: DemoState): HTMLElement {
  return el(
    'span',
    { class: `engine-badge engine-${s.engine.kind}`, title: s.engine.provenance },
    s.engine.kind === 'mock' ? 'MOCK ENGINE' : 'WASM ENGINE',
  );
}

export function renderIdentity(s: DemoState): HTMLElement {
  if (!s.identity) return el('div', { class: 'identity none' }, 'none');
  return el(
    'div',
    { class: 'identity' },
    `${shortAddress(s.identity.address)} · keyId ${s.identity.keyIdPrefix}…`,
    el(
      'span',
      { class: s.scanHits === 0 ? 'ok' : 'fail' },
      s.scanHits === 0 ? ' · key in 0 requests' : ` · key in ${s.scanHits} REQUESTS`,
    ),
  );
}
