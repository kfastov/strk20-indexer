# strk20 wallet — clickable UX prototype

A **UX sketch**, not a build. It exists so the interaction can be argued about
before the TypeScript package is written. It fetches nothing, verifies nothing
and discovers nothing: a mock engine sleeps for jittered intervals shaped like
the phases the real client will have, and reports the wall clock it actually
spent asleep.

**Every number on the screen is invented.** A fixed `SIMULATED mock-engine` chip
sits in the top-right corner, and the word is watermarked into the log's
background so no crop of a screenshot can lose it.

The page carries no explanatory copy at all: **the log is for events, the panels
are for state.** Nothing on screen explains the product, justifies a design choice
or argues a case. Everything below is documentation, not UI text — if you find
yourself wanting to move a paragraph from here onto the page, don't.

The test every log line has to pass: *would a person reading this learn that
something happened?* If the honest answer is "it tells them the machinery is
running", it does not get a line. So there is no `open`, no `subscribed`, and
above all no `check … no change`: a poll that found nothing is not an event. That
the subscription is alive is visible in its toggle, its cost in the requests
counter, and its effect in the feed panel's moving head.

```bash
npm install
npm run dev      # http://localhost:5180
```

No dependencies beyond Vite and TypeScript. `starknet.js` is deliberately absent:
nothing here signs, encodes or calls a chain, so it would have been decoration.

`npm run build` runs `tsc --noEmit` and then bundles; `npm run typecheck` alone.
Add `?seed=1234` to the URL to make the jitter reproducible.

## Layout

```
index.html            ALL markup, including the <template>s for log lines,
                      network rows and notes. Restructure the page here without
                      opening a .ts file; the contract is that the ids survive.
src/
  main.ts             wiring only — constructs the engine, binds events
  state.ts            the stage machine, the store, and every "why is this
                      disabled" selector
  format.ts           durations, bytes, felts, block numbers
  visibility.ts       background-tab timer-clamping guard (see Honesty below)
  wallet.ts           the WRITE path — deliberately outside the engine
  engine/
    types.ts          THE SEAM. The interface the real WASM client implements
    mock.ts           the one implementation today
    chain.ts          simulated Starknet + feed cutter (not part of the seam)
    fixtures.ts       the simulated feed, shaped from data/mainnet/feed
    latency.ts        seeded jitter, abortable sleeps
  ui/
    dom.ts            four helpers; everything else is plain DOM
    log.ts            the log, including the mutating last line
    panels.ts         cold/warm, feed, requests, network, identity, about
    controls.ts       the staged action bar
  styles.css
```

## The engine seam

`src/engine/types.ts` defines `Strk20Engine`. Everything the UI knows about
syncing goes through it:

| method | what the real client does |
|---|---|
| `open(identity)` | instantiate wasm, open the local store, report cold or warm |
| `sync({identity, mode, onPhase})` | load and apply feed bytes end to end, per-phase timings |
| `discover(identity)` | one discovery pass over the mirror already held |
| `network()` | every request issued, verbatim — the privacy evidence |
| `subscribe(handler)` | feed pokes; real transport is SSE `/feed/live` |
| `setLane(lane)` | `epochs` (ships today) or `snapshot` (planned) |
| `clearLocalState` / `feed` / `close` | |

To drop in the real client:

1. add `src/engine/wasm.ts` implementing `Strk20Engine`;
2. change the single `createEngine` binding in `src/main.ts` (marked
   `THE ONE BINDING`);
3. change nothing else.

`info.name` and `info.simulated` render in the corner chip, so a screenshot can
never pass a mock number off as a measurement of the real engine.

Two things are deliberately **not** on the seam:

- **`chain.ts`** — the simulated Starknet. The engine reads it; the wallet writes
  to it. In reality nobody in this project controls it.
- **`wallet.ts`** — deposit, send and withdraw. strk20-indexer has no write path
  and is not getting one (`docs/roadmap.md`, "Deferred, with triggers"): signing,
  key custody and a prover are the surface the project exists to avoid. Those
  buttons stand in for the privacy SDK plus the **hosted** prover, which also
  mints the FPI deposit-screening attestation — which is why a self-hosted prover
  can do everything except shield.

## Interaction states

Stage machine (`src/state.ts`):

```
boot → cold → syncing → ready ⇄ acting → waiting → ready
                                                 ↘ error
```

- **cold** — no mirror for this key; every action prints
  `no local mirror yet — run a sync first`.
- **acting** — one step of a staged action is in flight.
- **waiting** — submitted; a baseline of note ids and spent nullifiers is
  captured at arm time, and the pending log line mutates until a discovery pass
  sees something that is not in that baseline. Resolution is a set difference,
  not "the count went up". There is no timeout, only a cancel — a timeout would
  produce a number that looks like a measurement and is not one.

Staged actions are two steps each, approve-before-swap:

| action | step 1 | step 2 | gate |
|---|---|---|---|
| Deposit | approve STRK | shield | always available once synced |
| Send | select note | prove & send | needs an unspent note ≥ 25 STRK |
| Withdraw | select note | unshield | needs an unspent note ≥ 10 STRK |

Locked controls **say why** underneath — terse (`no note yet`,
`note 12.50 < 25.00 STRK`) — rather than going quietly grey, and step 2 renders
as locked until step 1 is green.

## What is grounded, and what is invented

Grounded in this repo (`data/mainnet/feed`, `docs/`):

- feed layout and URLs — `genesis.json`, `manifest.json`,
  `epochs/{e:08}.strk20e.zst`, `head.ndjson`, `anchors.ndjson`;
- epoch range 897…1414 (518 files), 15,817,408 bytes of epochs,
  16.0 MB total, 521 GETs for a cold start;
- the deployed mainnet pool address and chain id;
- the phase names and the `verified` grade vocabulary
  (`replayed` / `anchored` / `server-asserted`, `docs/spec/consumer-path.md` §1.5.1).

Invented:

- **every duration.** The cold ≈ 6 s / warm ≈ 0.03 s shape comes from a
  **native Rust CLI** run (`docs/pitch.md`, mainnet, 2026-08-31). Fold time
  *inside a browser* has never been measured — `docs/roadmap.md` calls it "the
  remaining sizing question";
- per-epoch byte sizes (a seeded curve normalised to the real total);
- note ids, nullifiers, amounts, addresses, and both viewing keys;
- the block clock, which runs about 5× faster than mainnet so the demo is
  clickable. Any time-to-discovery on screen is therefore not a latency claim,
  and the log says so each time one resolves.

Marked planned, not shipped: snapshots (roadmap item 1 —
`manifest.snapshot` is `null` in the real feed and none has ever been cut), SSE
`/feed/live` (item 2), the WASM engine and the npm client (items 3–4).
The snapshot lane's **request count** is arithmetic from the spec; its **byte
count** is left blank rather than invented.

## Honesty mechanisms

All of these are a chip, a word, or a colour — never a sentence.

- fixed `SIMULATED mock-engine` chip, plus the watermark inside the log;
- `visibility.ts` detects that the tab was backgrounded during a timed run —
  browser timers clamp to ~1 Hz there, which inflates every phase — and logs
  `tab hidden · timings clamped`, repeating it in amber under the cold column;
- a second cold run in the same session logs `http cache · epochs served locally`,
  because a page cannot clear that cache;
- the `plant key` button injects a compat-mode URL carrying the viewing key; the
  scanner line flips to red and back. A scanner that has never caught anything
  proves nothing;
- the `snapshot start` lane is tagged `planned` and flips the feed's `verified`
  row to `server-asserted`. One word each.

## Known simplifications

- No note joining, so after a couple of spends the wallet can hold only small
  notes and Send/Withdraw lock with an explanatory reason. Deposit again to clear it.
- No 10-block note maturity window (it would be a 60 s wait under this clock).
- No reorg, no `FEED_HASH_MISMATCH`, no `HISTORY_UNAVAILABLE` — all of which the
  real client has to surface and none of which has a designed UI yet.
- A manual check that finds nothing writes no log line, so the only feedback is
  the button reading `checking…` while the pass runs. If that turns out to be too
  quiet, the fix is a busier button, not a log line.
- State is in memory: a reload starts over. There is no IndexedDB here, so
  "warm" means "same page session".
