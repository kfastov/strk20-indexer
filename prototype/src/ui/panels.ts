/**
 * Every panel on the right-hand rail. Pure rendering: reads `AppState`, writes
 * the DOM. Never calls the engine, never mutates state.
 *
 * Copy rule: panels show numbers and results. No sentences, no provenance, no
 * justification. If a value needs a paragraph to be understood, it is the wrong
 * value to show.
 */

import { FIRST_EPOCH, IDENTITIES, LAST_EPOCH } from '../engine/fixtures';
import type { NetworkRecord, Phase } from '../engine/types';
import { balance, type AppState } from '../state';
import * as f from '../format';
import { all, clone, need, renderKv, setText } from './dom';

export class Panels {
  private coldwarm = need('#panel-coldwarm');
  private feedKv = need('#feed-kv');
  private identKv = need('#ident-kv');
  private notesList = need('#notes-list');
  private netList = need('#net-list');

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
    setText(document, '#engine-name', s.engine.name);
    const chip = need('#engine-chip');
    chip.classList.toggle('chip-real', !s.engine.simulated);
  }

  // -------------------------------------------------------------------------
  // cold vs warm
  // -------------------------------------------------------------------------

  private renderColdWarm(s: AppState): void {
    this.renderColumn('cold', s.coldTiming, s);
    this.renderColumn('warm', s.warmTiming, s);
    const caveat = need('#coldwarm-caveat');
    caveat.hidden = s.coldCaveat === null;
    caveat.textContent = s.coldCaveat ?? '';
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
      total.textContent = '—';
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
      ['epochs', `${FIRST_EPOCH} … ${LAST_EPOCH}  (${s.feed.epochCount})`],
      ['feed size', f.bytes(s.feed.feedBytes)],
      ['snapshot', s.feed.snapshotAvailable ? 'available' : 'null'],
      ['verified', grade, `v-${grade}`],
    ]);

    const on = s.subscription;
    setText(document, '#sub-state', on ? 'on' : 'off');
    need('#sub-state').dataset['on'] = String(on);
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
      li.textContent = '—';
      this.netList.append(li);
    }

    setText(document, '#btn-expand-net', s.netExpanded ? 'collapse' : 'expand');

    // verdict chip
    const chip = need('#net-verdict');
    const a = s.captures.A;
    const b = s.captures.B;
    if (a && b) {
      const same = a.length === b.length && a.every((u, i) => u === b[i]);
      chip.textContent = same ? 'A ≡ B' : 'A ≠ B';
      chip.dataset['state'] = same ? 'ok' : 'bad';
    } else {
      chip.textContent = '—';
      chip.dataset['state'] = 'pending';
    }

    // scanner
    const line = need('#scanner-line');
    const urls = s.network.map((r) => r.url).join('\n');
    const scanned = s.network.length;
    const hits = countKeyEncodings(urls);
    line.dataset['state'] = hits === 0 ? 'ok' : 'bad';
    line.textContent = `key: ${hits} hits / ${scanned} URLs`;
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
      ['viewing key', id.viewingKey, 'v-key'],
      ['balance', f.strk(bal)],
      ['notes', `${s.notes.filter((n) => !n.spent).length} unspent / ${s.notes.length} total`],
    ]);

    this.notesList.replaceChildren();
    if (s.notes.length === 0) {
      const p = document.createElement('p');
      p.className = 'notes-empty';
      p.textContent = 'no notes';
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
    const a = s.captures.A;
    const b = s.captures.B;

    if (!a || !b) {
      box.dataset['state'] = 'pending';
      head.textContent = a ? 'A captured' : b ? 'B captured' : '—';
      return;
    }

    const same = a.length === b.length && a.every((u, i) => u === b[i]);
    box.dataset['state'] = same ? 'ok' : 'bad';
    head.textContent = same ? `A ≡ B · ${a.length} URLs` : firstDiff(a, b);
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
