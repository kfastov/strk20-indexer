/**
 * Fake latency that behaves like latency.
 *
 * Fixed sleeps read as fake the moment you run the demo twice, so everything
 * here is jittered around a centre with a long-ish right tail — network work is
 * never symmetric. All of it is seeded so a run is reproducible when you want
 * it to be (`?seed=` in the URL) and lively when you don't.
 */

/** mulberry32 — small, fast, good enough, and deterministic. */
export function makeRng(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

export interface Jitter {
  /** Centre of the distribution, in ms. */
  readonly centre: number;
  /** Fractional spread, e.g. 0.25 => roughly ±25% before the tail. */
  readonly spread: number;
  /** Chance of a slow outlier, e.g. a stalled connection. */
  readonly tailChance?: number;
  /** Multiplier applied when the tail fires. */
  readonly tailFactor?: number;
}

export class Latency {
  private rng: () => number;

  constructor(seed: number) {
    this.rng = makeRng(seed);
  }

  random(): number {
    return this.rng();
  }

  /** A plausible duration in ms for a jittered operation. */
  draw(j: Jitter): number {
    // Triangular-ish: average two uniforms so the centre is actually likely.
    const u = (this.rng() + this.rng()) / 2;
    let ms = j.centre * (1 + (u * 2 - 1) * j.spread);
    if (j.tailChance && this.rng() < j.tailChance) {
      ms *= 1 + this.rng() * ((j.tailFactor ?? 2) - 1);
    }
    return Math.max(1, ms);
  }

  /** Pick uniformly from a range. */
  between(lo: number, hi: number): number {
    return lo + this.rng() * (hi - lo);
  }

  pick<T>(xs: readonly T[]): T {
    const i = Math.min(xs.length - 1, Math.floor(this.rng() * xs.length));
    return xs[i] as T;
  }
}

export class Aborted extends Error {
  constructor() {
    super('aborted');
    this.name = 'Aborted';
  }
}

/** Abortable sleep. Rejects with `Aborted` so callers can distinguish it. */
export function sleep(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) return reject(new Aborted());
    const t = window.setTimeout(() => {
      signal?.removeEventListener('abort', onAbort);
      resolve();
    }, ms);
    const onAbort = () => {
      window.clearTimeout(t);
      reject(new Aborted());
    };
    signal?.addEventListener('abort', onAbort, { once: true });
  });
}

/**
 * Sleep for `total` ms while calling `onTick` roughly every `step` ms with the
 * fraction elapsed. Used so a six-second phase is not a dead six seconds.
 */
export async function sleepProgress(
  total: number,
  onTick: (fraction: number, elapsed: number) => void,
  signal?: AbortSignal,
  step = 90,
): Promise<void> {
  const started = performance.now();
  for (;;) {
    const elapsed = performance.now() - started;
    if (elapsed >= total) break;
    onTick(Math.min(1, elapsed / total), elapsed);
    await sleep(Math.min(step, total - elapsed), signal);
  }
  onTick(1, performance.now() - started);
}
