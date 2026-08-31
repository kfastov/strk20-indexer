/**
 * Timer-clamping guard.
 *
 * A backgrounded tab has its timers clamped to ~1 Hz, so every "measurement"
 * taken while the tab was hidden is inflated — often by several seconds. In a
 * prototype whose entire premise is that no number may be passed off as a
 * measurement, silently reporting those inflated numbers would be the exact
 * failure mode the banner exists to prevent.
 *
 * So: record when the tab was hidden, and let callers ask whether any interval
 * they timed overlapped one of those periods.
 */

type Interval = [start: number, end: number];

export class VisibilityWatch {
  private hiddenIntervals: Interval[] = [];
  private hiddenSince: number | null = null;

  constructor() {
    if (document.hidden) this.hiddenSince = performance.now();
    document.addEventListener('visibilitychange', () => {
      if (document.hidden) {
        this.hiddenSince = performance.now();
      } else if (this.hiddenSince !== null) {
        this.hiddenIntervals.push([this.hiddenSince, performance.now()]);
        this.hiddenSince = null;
        // Keep only what a long session could plausibly still ask about.
        if (this.hiddenIntervals.length > 200) this.hiddenIntervals.splice(0, 100);
      }
    });
  }

  /** True if the tab was hidden at any point in [from, to]. */
  wasHiddenDuring(from: number, to: number): boolean {
    if (this.hiddenSince !== null && this.hiddenSince <= to) return true;
    return this.hiddenIntervals.some(([s, e]) => s < to && e > from);
  }
}
