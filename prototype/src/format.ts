/** Formatting only. No state, no DOM. */

export function ms(v: number): string {
  if (v < 1) return '<1 ms';
  if (v < 1000) return `${Math.round(v)} ms`;
  if (v < 10_000) return `${(v / 1000).toFixed(2)} s`;
  return `${(v / 1000).toFixed(1)} s`;
}

/** Live-ticking duration for the pending line: coarser, so it does not strobe. */
export function tick(v: number): string {
  if (v < 1000) return `${Math.round(v / 10) * 10} ms`;
  return `${(v / 1000).toFixed(1)} s`;
}

export function bytes(v: number): string {
  if (v === 0) return '0 B';
  if (v < 1024) return `${Math.round(v)} B`;
  if (v < 1e6) return `${(v / 1000).toFixed(1)} kB`;
  return `${(v / 1e6).toFixed(1)} MB`;
}

export function count(v: number): string {
  return v.toLocaleString('en-US');
}

export function strk(v: number): string {
  return `${v.toFixed(2)} STRK`;
}

export function shortFelt(f: string, head = 8, tail = 4): string {
  return f.length > head + tail + 2 ? `${f.slice(0, head)}…${f.slice(-tail)}` : f;
}

export function clock(d = new Date()): string {
  return d.toTimeString().slice(0, 8);
}

export function block(n: number): string {
  return n.toLocaleString('en-US').replace(/,/g, ' ');
}
