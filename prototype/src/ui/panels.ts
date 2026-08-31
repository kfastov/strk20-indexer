/**
 * Every panel on the right-hand rail. Pure rendering: this module reads
 * `AppState` and writes the DOM. It never calls the engine and never mutates
 * state — `main.ts` owns both.
 */

import {
  EPOCH_COUNT,
  FIRST_EPOCH,
  IDENTITIES,
  LAST_EPOCH,
  SNAPSHOT_LANE_REQUESTS,
} from '../engine/fixtures';
import type { NetworkRecord, Phase } from '../engine/types';
import { balance, type AppState } from '../state';
import * as f from '../format';
import { all, clone, need, renderKv, setText } from './dom';

/** Cold-lane request arithmetic: genesis + manifest + N epochs + head. */
const EPOCH_LANE_REQUESTS = 2 + EPOCH_COUNT + 1;

export class Panels {
  private coldwarm = need('#panel-coldwarm');
  private feedKv = need('#feed-kv');
  private identKv = need('#ident-kv');
  private notesList = need('#notes-list');
  private netList = need('#net-list');
  private aboutList = need('#about-list');

  constructor() {
    this.renderAbout();
  }

  render(s: AppState): void {
    this.renderEngine(s);
    this.renderColdWarm(s);
    this.renderFeed(s);
    this.renderRequests(s);
    this.renderNetwork(s);
    this.renderIdentity(s);
  }

  // -------------------------------------------------------------------------

  private renderEngine(s: AppState): void {
    setText(document, '#engine-name', `${s.engine.name} — ${s.engine.notice}`);
    const chip = need('#engine-sim');
    chip.textContent = s.engine.simulated ? 'SIMULATED' : 'REAL ENGINE';
    chip.classList.toggle('chip-real', !s.engine.simulated);
  }

  // -------------------------------------------------------------------------
  // cold vs warm
  // -------------------------------------------------------------------------

  private renderColdWarm(s: AppState): void {
    this.renderColumn('cold', s.coldTiming, s);
    this.renderColumn('warm', s.warmTiming, s);
    const note = need('#coldwarm-provenance');
    note.classList.toggle('has-caveat', s.coldCaveat !== null);
    const existing = note.querySelector('.caveat');
    existing?.remove();
    if (s.coldCaveat) {
      const b = document.createElement('span');
      b.className = 'caveat';
      b.textContent = s.coldCaveat;
      note.prepend(b);
    }
  }

  private renderColumn(col: 'cold' | 'warm', timing: AppState['coldTiming'], s: AppState): void {
    const root = need(`[data-col="${col}"]`, this.coldwarm);
    const total = need('[data-role="total"]', root);
    const phases = need('[data-role="phases"]', root);
    const running = s.stage.s === 'syncing' && s.stage.mode === col;

    if (running) {
      total.innerHTML = '<span class="cw-running">running…</span>';
      phases.replaceChildren();
      return;
    }
    if (!timing) {
      total.innerHTML = 'not measured<br /><span class="cw-total-sub">in this session</span>';
      phases.replaceChildren();
      return;
    }

    total.textContent = f.ms(timing.totalMs);
    phases.replaceChildren();
    for (const p of timing.phases) {
      phases.append(phaseRow(p));
    }
  }

  // -------------------------------------------------------------------------
  // feed
  // -------------------------------------------------------------------------

  private renderFeed(s: AppState): void {
    const grade = s.feed.verified;
    renderKv(this.feedKv, [
      ['chain', s.feed.chainId],
      ['head', f.block(s.feed.head)],
      ['l1 accepted', f.block(s.feed.l1Accepted)],
      ['epochs', `${FIRST_EPOCH} … ${LAST_EPOCH}  (${EPOCH_COUNT})`],
      ['feed size', f.bytes(s.feed.feedBytes)],
      ['snapshot', s.feed.snapshotAvailable ? 'available' : 'null — never cut'],
      ['verified', grade, `v-${grade}`],
    ]);

    const on = s.subscription;
    setText(document, '#sub-state', on ? 'on' : 'off');
    need('#sub-state').dataset['on'] = String(on);
    need('#sub-note').innerHTML = on
      ? 'discovery re-runs on every feed poke, and each poke costs one conditional GET. ' +
        'The real transport is <b>SSE <code>/feed/live</code> — roadmap item 2, unbuilt</b>. ' +
        'Head advances ~5&times; faster here than mainnet so the demo is clickable.'
      : 'nothing runs by itself. Use <b>check now</b> in the bar below; the elapsed time is logged either way, ' +
        'labelled <code>manual</code> so it is never read as a subscription latency.';
  }

  // -------------------------------------------------------------------------
  // requests
  // -------------------------------------------------------------------------

  private renderRequests(s: AppState): void {
    const n = s.network.length;
    const bytes = s.network.reduce((a, r) => a + r.bytes, 0);
    setText(document, '#req-count', f.count(n));
    setText(document, '#req-bytes', f.bytes(bytes));

    for (const btn of all<HTMLButtonElement>('.lane')) {
      const active = btn.dataset['lane'] === s.lane;
      btn.setAttribute('aria-checked', String(active));
      btn.classList.toggle('is-active', active);
    }

    const delta = need('#req-delta');
    const note = need('#req-note');
    if (s.lane === 'epochs') {
      delta.innerHTML =
        `<b>${EPOCH_LANE_REQUESTS}</b> GETs for a full cold start today ` +
        `<span class="arrow">→</span> <b>${SNAPSHOT_LANE_REQUESTS}</b> with snapshots`;
      note.innerHTML =
        `genesis + manifest + ${EPOCH_COUNT} epochs + head. The measured figure in ` +
        `<code>docs/pitch.md</code> is <b>518 requests / 16 MB</b>, from a native Rust CLI run on ` +
        `2026-08-31 when the feed held 515 epochs — it has grown three epochs since. ` +
        `Cost is paid once per cold start, then ~80 kB/day.`;
    } else {
      delta.innerHTML =
        `<b>${SNAPSHOT_LANE_REQUESTS}</b> GETs, independent of how long the pool has existed`;
      note.innerHTML =
        `genesis + manifest + snapshot + anchor + 0–1 epochs + head. ` +
        `<b>Planned, not built</b> (roadmap item 1): <code>manifest.snapshot</code> is <code>null</code> in the ` +
        `real feed and no snapshot has ever been cut, so the request count is arithmetic from the spec and ` +
        `the byte figure is left blank rather than invented. Snapshot start also carries a ` +
        `<em>lower</em> integrity grade (<code>server-asserted</code>) unless you point it at your own RPC.`;
    }
  }

  // -------------------------------------------------------------------------
  // network
  // -------------------------------------------------------------------------

  private renderNetwork(s: AppState): void {
    this.netList.replaceChildren();

    const epochs = s.network.filter((r) => r.url.startsWith('/feed/epochs/'));
    const shown: NetworkRecord[] = s.netExpanded
      ? [...s.network]
      : s.network.filter((r) => !r.url.startsWith('/feed/epochs/'));

    let inserted = false;
    for (const r of shown) {
      // Keep the group row where the epochs actually were.
      if (!s.netExpanded && !inserted && epochs.length > 0 && r.url === '/feed/head.ndjson') {
        this.netList.append(epochGroupRow(epochs));
        inserted = true;
      }
      this.netList.append(netRow(r));
    }
    if (!s.netExpanded && !inserted && epochs.length > 0) {
      this.netList.append(epochGroupRow(epochs));
    }
    if (s.network.length === 0) {
      const li = document.createElement('li');
      li.className = 'net-empty';
      li.textContent = 'nothing fetched yet';
      this.netList.append(li);
    }

    setText(document, '#btn-expand-net', s.netExpanded ? 'collapse epoch rows' : 'expand all rows');

    // verdict chip
    const chip = need('#net-verdict');
    const a = s.captures.A;
    const b = s.captures.B;
    if (a && b) {
      const same = a.length === b.length && a.every((u, i) => u === b[i]);
      chip.textContent = same ? 'A ≡ B' : 'A ≠ B';
      chip.dataset['state'] = same ? 'ok' : 'bad';
    } else {
      chip.textContent = 'A vs B: not compared';
      chip.dataset['state'] = 'pending';
    }

    // scanner
    const line = need('#scanner-line');
    const urls = s.network.map((r) => r.url).join('\n');
    const scanned = s.network.length;
    const hits = countKeyEncodings(urls);
    if (hits === 0) {
      line.dataset['state'] = 'ok';
      line.textContent =
        `viewing key: no encoding of either wallet's key found across ${scanned} request URLs; ` +
        `every request is a GET with an empty body`;
    } else {
      line.dataset['state'] = 'bad';
      line.textContent =
        `viewing key: found, in ${hits} encoding${hits === 1 ? '' : 's'} — the planted compat-mode request above. ` +
        `The feed lane cannot emit that URL; there is no engine method that takes a key.`;
    }
  }

  // -------------------------------------------------------------------------
  // identity
  // -------------------------------------------------------------------------

  private renderIdentity(s: AppState): void {
    const id = IDENTITIES[s.identityId];
    for (const btn of all<HTMLButtonElement>('.ident')) {
      const active = btn.dataset['identity'] === s.identityId;
      btn.setAttribute('aria-checked', String(active));
      btn.classList.toggle('is-active', active);
    }

    const bal = balance(s.notes);
    renderKv(this.identKv, [
      ['address', f.shortFelt(id.address, 10, 6)],
      ['viewing key', `${id.viewingKey}  · never leaves this tab`, 'v-key'],
      ['balance', f.strk(bal)],
      ['notes', `${s.notes.filter((n) => !n.spent).length} unspent / ${s.notes.length} total`],
    ]);

    this.notesList.replaceChildren();
    if (s.notes.length === 0) {
      const p = document.createElement('p');
      p.className = 'notes-empty';
      p.textContent =
        s.stage.s === 'cold' || s.stage.s === 'boot'
          ? 'no discovery run yet'
          : 'no notes for this key in the feed';
      this.notesList.append(p);
    }
    for (const n of s.notes) {
      const el = clone<HTMLElement>('tpl-note');
      el.dataset['spent'] = String(n.spent);
      setText(el, '.note-id', f.shortFelt(n.id, 8, 4));
      setText(el, '.note-amount', f.strk(n.amount));
      setText(el, '.note-block', `blk ${f.block(n.block)}`);
      setText(el, '.note-state', n.spent ? 'spent' : 'unspent');
      this.notesList.append(el);
    }

    this.renderVerdict(s);
  }

  private renderVerdict(s: AppState): void {
    const box = need('#ident-verdict');
    const head = need('#ident-verdict-head');
    const body = need('#ident-verdict-body');
    const a = s.captures.A;
    const b = s.captures.B;

    if (!a || !b) {
      box.dataset['state'] = 'pending';
      head.textContent = a || b ? 'one wallet captured — sync the other' : 'run both wallets to compare';
      body.textContent =
        'sync wallet A, then switch to wallet B and sync. The request list is captured for each and ' +
        'compared URL by URL, in order.';
      return;
    }

    const same = a.length === b.length && a.every((u, i) => u === b[i]);
    box.dataset['state'] = same ? 'ok' : 'bad';
    if (same) {
      head.textContent = `verdict: identical — ${a.length} URLs, same order, both wallets`;
      body.innerHTML =
        'The server saw the same bytes leave and the same bytes come back for two different keys. ' +
        'It cannot tell the two wallets apart, because nothing about them was ever in a request. ' +
        'The discovered notes differ; the traffic does not. ' +
        '<em>In the real build this is a CI assertion over a recording proxy, not a claim.</em>';
    } else {
      head.textContent = 'verdict: DIFFERENT — this would be a bug';
      body.textContent = firstDiff(a, b);
    }
  }

  // -------------------------------------------------------------------------
  // about
  // -------------------------------------------------------------------------

  private renderAbout(): void {
    const rows: ReadonlyArray<[string, 'shipped' | 'planned' | 'never', string]> = [
      ['keyless discovery from a public feed', 'shipped', 'Rust client, CI-asserted address-blind'],
      ['epoch bundles, hash-chained, content-addressed', 'shipped', '518 epochs on mainnet today'],
      ['storage-root verify at every cut', 'shipped', 'refuses to publish a divergent epoch'],
      ['snapshots / O(1) cold start', 'planned', 'roadmap item 1 — manifest.snapshot is null'],
      ['SSE /feed/live subscription', 'planned', 'roadmap item 2 — polling is what exists'],
      ['WASM engine in the browser', 'planned', 'spike done (231 kB gzip); items 3–4 unbuilt'],
      ['npm KeylessClient / DelegatedClient', 'planned', 'roadmap item 4'],
      ['browser fold time for full history', 'planned', 'never measured — the open sizing question'],
      ['deposit / send / withdraw', 'never', 'no write path, deliberately cut; the wallet + hosted prover own it'],
    ];
    this.aboutList.replaceChildren();
    for (const [what, state, why] of rows) {
      const li = document.createElement('li');
      li.className = 'about-item';
      li.dataset['state'] = state;
      li.innerHTML = `<span class="about-state">${state}</span><span class="about-what">${what}</span><span class="about-why">${why}</span>`;
      this.aboutList.append(li);
    }
  }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

function phaseRow(p: Phase): HTMLElement {
  const row = document.createElement('div');
  row.className = 'cw-phase';
  if (p.skipped) row.dataset['skipped'] = 'true';
  const dt = document.createElement('dt');
  dt.textContent = p.name;
  const dd = document.createElement('dd');
  if (p.skipped) {
    dd.textContent = p.skippedBytes ? `−${f.bytes(p.skippedBytes)}` : 'not run';
    dd.title = p.detail ?? 'not performed on this path';
  } else {
    dd.textContent = f.ms(p.ms);
    if (p.detail) dd.title = p.detail;
  }
  row.append(dt, dd);
  return row;
}

function netRow(r: NetworkRecord): HTMLElement {
  const el = clone<HTMLElement>('tpl-net-row');
  if (r.synthetic) {
    el.dataset['synthetic'] = 'true';
    el.title = r.synthetic;
  }
  setText(el, '.net-url', r.url);
  const status = need('.net-status', el);
  status.textContent = String(r.status);
  status.dataset['status'] = r.status === 200 ? 'ok' : 'other';
  setText(el, '.net-bytes', r.bytes === 0 ? '—' : f.bytes(r.bytes));
  if (r.source === 'http-cache') el.dataset['source'] = 'cache';
  return el;
}

function epochGroupRow(epochs: readonly NetworkRecord[]): HTMLElement {
  const el = clone<HTMLElement>('tpl-net-group');
  const bytes = epochs.reduce((a, r) => a + r.bytes, 0);
  setText(
    el,
    '.net-url',
    `/feed/epochs/${String(FIRST_EPOCH).padStart(8, '0')} … ${String(LAST_EPOCH).padStart(8, '0')}.strk20e.zst`,
  );
  setText(el, '.net-bytes', `${epochs.length} GETs · ${f.bytes(bytes)}`);
  return el;
}

/**
 * A stand-in for the byte scanner the real acceptance test runs, reduced to the
 * URL surface a page can see. It reports how many ENCODINGS of a viewing key
 * matched rather than raw substring counts, because "0xa11ce" trivially
 * contains "a11ce" and a doubled count would look like a doubled leak.
 *
 * The real scanner covers minimal hex, padded hex, hex without 0x, uppercase,
 * decimal ASCII, raw 32-byte BE and LE, and base64 — over full request bytes,
 * not just URLs — and is proven non-vacuous against a compat-mode body.
 */
function countKeyEncodings(haystack: string): number {
  let matched = 0;
  for (const id of Object.values(IDENTITIES)) {
    const bare = id.viewingKey.replace(/^0x/, '');
    const encodings = [
      id.viewingKey,
      bare.padStart(64, '0'),
      bare.toUpperCase(),
      BigInt(id.viewingKey).toString(10),
    ];
    for (const e of encodings) {
      // Short decimal forms of these toy keys could collide with block numbers;
      // the real keys are 251-bit and this guard is unnecessary there.
      if (e.length < 5) continue;
      if (haystack.includes(e)) matched++;
    }
  }
  return matched;
}

function firstDiff(a: readonly string[], b: readonly string[]): string {
  const n = Math.max(a.length, b.length);
  for (let i = 0; i < n; i++) {
    if (a[i] !== b[i]) return `first difference at request ${i + 1}: A=${a[i] ?? '—'}  B=${b[i] ?? '—'}`;
  }
  return 'lengths differ';
}
