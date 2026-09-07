/** Public transport state only. The reconstructed artifact still goes through
 * the existing Rust parser and independent checkpoint/root verification. */
export interface LiveHead {
  payload: string | null;
  etag: string;
  head: number;
  head_hash: string;
  l1_accepted: number;
  tail_from: number;
  resync: boolean;
  delta?: { base_etag: string; header: string; append: string; end: string };
}

const MAX_INLINE = 2 * 1024 * 1024;

export class LiveHeadAssembler {
  private current: LiveHead | undefined;

  reset(): void { this.current = undefined; }

  receive(update: LiveHead): LiveHead {
    let payload = update.payload;
    if (update.delta) {
      const base = this.current;
      const { base_etag, header, append, end } = update.delta;
      const old = base?.payload;
      if (!old || base.etag !== base_etag || base.tail_from !== update.tail_from
        || typeof header !== "string" || typeof append !== "string" || typeof end !== "string"
        || !header.endsWith("\n") || !end.endsWith("\n")
        || (append !== "" && !append.endsWith("\n"))) return this.gap();
      const first = old.indexOf("\n") + 1;
      const last = old.lastIndexOf("\n", old.length - 2) + 1;
      if (!first || last < first || !old.endsWith("\n")) return this.gap();
      payload = header + old.slice(first, last) + append + end;
    }
    if (payload != null && (typeof payload !== "string"
      || payload.length > MAX_INLINE || new TextEncoder().encode(payload).length > MAX_INLINE))
      return this.gap();
    const assembled = { ...update, payload };
    delete assembled.delta;
    this.current = payload ? assembled : undefined;
    return assembled;
  }

  private gap(): never {
    this.reset();
    throw new Error("FEED_LIVE_GAP: head delta has no matching base; reconnect for current state");
  }
}
