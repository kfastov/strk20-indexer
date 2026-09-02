/**
 * The live stream pokes on the events the server actually sends.
 *
 * `crates/indexerd/src/live.rs` names every event it writes — `hello`, `head`,
 * `epoch`, `snapshot`, `status` (consumer-path.md §2.2) — and `EventSource`
 * delivers a named event ONLY to a listener registered under that name.
 * `openLive` hooked `es.onmessage`, which receives unnamed events, so against
 * this repository's own feed the subscription connected, never poked, and
 * never errored: no error means no fall back to polling, so `subscription: ON`
 * sat there inert. Measured in a browser against the live mainnet stream over
 * 70 s before the fix: `{onmessage: 0, head: 5, hello: 1, error: 0}`.
 *
 * The stub below is the shape of that wire, not of the old handler.
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { openLive, type NetContext } from '../src/net.ts';

class StubEventSource {
  static last: StubEventSource | null = null;
  onopen: (() => void) | null = null;
  onmessage: ((ev: { data: string }) => void) | null = null;
  onerror: (() => void) | null = null;
  closed = false;
  readonly listeners = new Map<string, Array<(ev: { data: string }) => void>>();
  readonly url: string;

  constructor(url: string) {
    this.url = url;
    StubEventSource.last = this;
  }

  addEventListener(name: string, fn: (ev: { data: string }) => void): void {
    const list = this.listeners.get(name) ?? [];
    list.push(fn);
    this.listeners.set(name, list);
  }

  close(): void {
    this.closed = true;
  }

  /** `event: <name>\ndata: <data>` off the wire. */
  emit(name: string, data: string): void {
    for (const fn of this.listeners.get(name) ?? []) fn({ data });
  }

  /** An event with no `event:` line, which is what `onmessage` is for. */
  emitUnnamed(data: string): void {
    this.onmessage?.({ data });
  }
}

function ctx(): NetContext {
  return {
    fetchImpl: (() => {
      throw new Error('openLive must not fetch');
    }) as unknown as NetContext['fetchImpl'],
    onRecord: () => {},
    now: () => 0,
  };
}

function withStub<T>(fn: () => T): T {
  const g = globalThis as { EventSource?: unknown };
  const saved = g.EventSource;
  g.EventSource = StubEventSource as unknown as typeof EventSource;
  try {
    return fn();
  } finally {
    g.EventSource = saved;
  }
}

test('the named events that announce new bytes each poke once', () => {
  withStub(() => {
    let pokes = 0;
    const live = openLive(ctx(), 'https://example.test/feed', { onPoke: () => pokes++, onError: () => {} });
    const es = StubEventSource.last!;
    assert.equal(es.url, 'https://example.test/feed/live');

    es.emit('head', '{"head":14268422}');
    es.emit('epoch', '{"e":1425}');
    es.emit('snapshot', '{"e":1425}');
    assert.equal(pokes, 3);

    // Still honoured, for a mirror that publishes unnamed events.
    es.emitUnnamed('{"head":14268423}');
    assert.equal(pokes, 4);
    live.close();
  });
});

test('hello and status are received but announce no artifact, so they do not poke', () => {
  withStub(() => {
    let pokes = 0;
    const live = openLive(ctx(), 'https://example.test/feed', { onPoke: () => pokes++, onError: () => {} });
    const es = StubEventSource.last!;
    es.emit('hello', '{"chain_id":"SN_MAIN"}');
    es.emit('status', '{"decode_state":"ok"}');
    assert.equal(pokes, 0);
    live.close();
  });
});

test('every event counts toward the panel row, poking or not', () => {
  withStub(() => {
    const live = openLive(ctx(), 'https://example.test/feed', { onPoke: () => {}, onError: () => {} });
    const es = StubEventSource.last!;
    es.emit('hello', 'abcde');
    es.emit('head', '1234567890');
    es.emitUnnamed('xy');
    // 5 + 10 + 2: the SSE connection is a row like any other (§6.2 rule 2).
    assert.equal(live.record.bytes, 17);
    live.close();
  });
});

test('an error still degrades rather than hanging', () => {
  withStub(() => {
    let errors = 0;
    const live = openLive(ctx(), 'https://example.test/feed', { onPoke: () => {}, onError: () => errors++ });
    StubEventSource.last!.onerror?.();
    assert.equal(errors, 1);
    live.close();
    assert.equal(StubEventSource.last!.closed, true);
  });
});
