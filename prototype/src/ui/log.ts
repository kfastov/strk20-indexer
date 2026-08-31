/**
 * The log, and the mutating last line.
 *
 * The behaviour this prototype exists to evaluate:
 *
 *   - the pending line is ALWAYS the last thing in the scroller. Lines that
 *     arrive while something is pending are inserted above it, so the thing you
 *     are waiting for never scrolls away from under your eyes;
 *   - it mutates in place: its detail text can be rewritten, and its duration
 *     column ticks at 10 Hz off one rAF loop shared by the whole view;
 *   - when it resolves it FREEZES into the log: the element is moved from the
 *     pending slot into the committed list, keeping its identity, with the
 *     elapsed time computed from two `performance.now()` stamps rather than
 *     from the tick counter (ticks drift; stamps do not);
 *   - the log carries events only. A discovery pass that found nothing never
 *     reaches this module, so there is no counter, no collapsing and no
 *     de-duplication to implement here.
 */

import { clock, ms, tick } from '../format';
import { need, prefersReducedMotion } from './dom';

export type LineKind =
  | 'info'
  | 'ok'
  | 'warn'
  | 'error'
  | 'net'
  | 'action'
  | 'privacy'
  | 'pending';

export interface LineSpec {
  readonly event: string;
  readonly detail: string;
  readonly kind?: LineKind;
  readonly durationMs?: number;
  /** Rendered greyed and italic — used for "this is a simulation" asides. */
  readonly aside?: boolean;
}

export interface PendingHandle {
  readonly active: boolean;
  readonly elapsedMs: number;
  update(detail: string, event?: string): void;
  /** Freezes the line into the log with its measured elapsed time. */
  resolve(spec: { event?: string; detail: string; kind?: LineKind }): number;
  fail(detail: string): number;
  /** Freezes it as cancelled, with the elapsed time it did accumulate. */
  cancel(detail?: string): number;
}

const TICK_MS = 100;
const DOT_FRAMES = ['', '.', '..', '...'];

export class LogView {
  private raf = 0;
  private lastTick = 0;
  private current: ActivePending | null = null;
  private reduced = prefersReducedMotion();

  constructor(
    private scroll: HTMLElement,
    private list: HTMLElement,
    private slot: HTMLElement,
  ) {
    window
      .matchMedia('(prefers-reduced-motion: reduce)')
      .addEventListener('change', (e) => (this.reduced = e.matches));
  }

  // -------------------------------------------------------------------------
  // committed lines
  // -------------------------------------------------------------------------

  append(spec: LineSpec): HTMLElement {
    const stick = this.atBottom();
    const el = buildLine(spec);
    this.list.append(el);
    this.flash(el);
    if (stick) this.toBottom();
    return el;
  }

  // -------------------------------------------------------------------------
  // the pending line
  // -------------------------------------------------------------------------

  pending(event: string, detail: string): PendingHandle {
    // Only one line may be alive at a time. Starting another commits the old
    // one rather than silently dropping it.
    this.current?.handle.cancel('superseded');

    const el = buildPending(event, detail);
    this.slot.replaceChildren(el);
    const active: ActivePending = {
      el,
      event,
      detail,
      startedAt: performance.now(),
      frame: 0,
      handle: null as unknown as PendingHandle,
    };
    active.handle = this.makeHandle(active);
    this.current = active;
    this.startTicker();
    this.toBottom();
    return active.handle;
  }

  private makeHandle(a: ActivePending): PendingHandle {
    const self = this;
    const finish = (spec: { event?: string; detail: string; kind?: LineKind }): number => {
      if (self.current !== a) return 0;
      const elapsed = performance.now() - a.startedAt;
      self.current = null;
      self.stopTicker();
      self.slot.replaceChildren();
      self.append({
        event: spec.event ?? a.event,
        detail: spec.detail,
        kind: spec.kind ?? 'ok',
        durationMs: elapsed,
      });
      return elapsed;
    };

    return {
      get active() {
        return self.current === a;
      },
      get elapsedMs() {
        return performance.now() - a.startedAt;
      },
      update(detail: string, event?: string) {
        if (self.current !== a) return;
        a.detail = detail;
        if (event !== undefined) {
          a.event = event;
          need('.line-event-text', a.el).textContent = event;
        }
        need('.line-detail-text', a.el).textContent = detail;
      },
      resolve: (spec) => finish(spec),
      fail: (detail) => finish({ detail, kind: 'error' }),
      cancel: (detail) => finish({ detail: detail ?? 'cancelled', kind: 'warn' }),
    };
  }

  // -------------------------------------------------------------------------
  // the ticker — one loop for the whole view
  // -------------------------------------------------------------------------

  private startTicker(): void {
    if (this.raf) return;
    this.lastTick = 0;
    const loop = (now: number) => {
      this.raf = requestAnimationFrame(loop);
      if (!this.current) return;
      if (now - this.lastTick < TICK_MS) return;
      this.lastTick = now;
      const a = this.current;
      const elapsed = performance.now() - a.startedAt;
      need('.line-dur', a.el).textContent = tick(elapsed);
      const dots = need('.dots', a.el);
      if (this.reduced) {
        dots.textContent = '…';
      } else {
        a.frame = (a.frame + 1) % DOT_FRAMES.length;
        dots.textContent = DOT_FRAMES[a.frame] ?? '';
      }
    };
    this.raf = requestAnimationFrame(loop);
  }

  private stopTicker(): void {
    if (this.raf) cancelAnimationFrame(this.raf);
    this.raf = 0;
  }

  // -------------------------------------------------------------------------
  // scrolling
  // -------------------------------------------------------------------------

  private atBottom(): boolean {
    return this.scroll.scrollHeight - this.scroll.scrollTop - this.scroll.clientHeight < 48;
  }

  private toBottom(): void {
    this.scroll.scrollTop = this.scroll.scrollHeight;
  }

  private flash(el: HTMLElement): void {
    if (this.reduced) return;
    el.classList.remove('line-new');
    void el.offsetWidth; // restart the animation
    el.classList.add('line-new');
  }

  clear(): void {
    this.current?.handle.cancel('cleared');
    this.list.replaceChildren();
    this.slot.replaceChildren();
  }
}

interface ActivePending {
  el: HTMLElement;
  event: string;
  detail: string;
  startedAt: number;
  frame: number;
  handle: PendingHandle;
}

function buildLine(spec: LineSpec): HTMLElement {
  const t = document.getElementById('tpl-log-line') as HTMLTemplateElement;
  const el = t.content.firstElementChild!.cloneNode(true) as HTMLElement;
  el.dataset['kind'] = spec.kind ?? 'info';
  if (spec.aside) el.dataset['aside'] = 'true';
  need('.line-time', el).textContent = clock();
  need('.line-event', el).textContent = spec.event;
  need('.line-detail', el).textContent = spec.detail;
  need('.line-dur', el).textContent = spec.durationMs === undefined ? '' : ms(spec.durationMs);
  return el;
}

function buildPending(event: string, detail: string): HTMLElement {
  const t = document.getElementById('tpl-pending-line') as HTMLTemplateElement;
  const el = t.content.firstElementChild!.cloneNode(true) as HTMLElement;
  need('.line-time', el).textContent = clock();
  need('.line-event-text', el).textContent = event;
  need('.line-detail-text', el).textContent = detail;
  need('.line-dur', el).textContent = '0 ms';
  return el;
}
