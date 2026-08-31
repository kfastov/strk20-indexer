# Demo application — specification

Status: FINAL for implementation. Companion to
[consumer-path.md](consumer-path.md) §A3/§A4; every `§n.n` reference below is to
that document unless it says otherwise. The judge conflicts this design resolves
are §0.5 S12–S17 there, summarised again in §11 here.

The demo is a **test target and a proof, not a slide**. Its Playwright suite runs
in CI against the REPLAY lane (§10), and every claim it makes on screen is
either measured in the session in front of you or labelled as a recorded
reference with its source line.

---

## 1. What it exists to prove, in order

1. **Cold is seconds, warm is instant — side by side**, not sequentially in a
   log where the contrast is lost.
2. **The network sees no key and no address — visibly**, by listing every URL
   the page fetched, and by showing that a second identity produces the same
   list.
3. **A note is discovered, and how long that took.**
4. **The request count and the byte count**, because the drop from ~518 requests
   to a handful once snapshots land is what we sell.

Everything else is decoration and is cut before any of these.

## 2. The positioning fact, and what it forbids

Verified from the official Wallet API docs: *"No viewing keys in your app. The
wallet holds the user's viewing key"* and *"The wallet discovers notes, builds
the proof"*. Two consequences bind this design:

- **A demo that "connects a wallet" and then discovers notes is not merely out
  of scope — it is impossible.** A Wallet-API wallet never hands over a viewing
  key. The demo is therefore honest about being **a wallet-shaped app that holds
  its own viewing key**, which is exactly our customer.
- **We have no write path, deliberately** (roadmap "Deferred"; design notes §4).
  The action buttons do the half we actually do. The framing is the project's
  own and is the strongest one available: **we are the read half of every write.
  You sign it; we tell you the moment your note exists, and later the moment it
  is spent.**

A non-dismissible banner carries the first fact at Stage 2, verbatim:

> This page holds a viewing key, because that is what our customer does — a
> wallet, or a key-holding app. A dapp on the Starknet Wallet API never receives
> a viewing key: the wallet holds it, discovers the notes and builds the proof.
> Such a dapp cannot use this library and does not need it.

## 3. Shape, lanes and layout

`ts/demo`, a single-page app, no framework, no bundler beyond `tsc` + `esbuild`,
importing `strk20-discovery` from the workspace. No third-party origins at all:
system font stacks, no analytics, no error reporting, no CDN.

**Relationship to the UI prototype in the tree.** `prototype/` holds a visual
sketch of this screen driven by a mock engine (`src/engine/mock.ts`,
`fixtures.ts`, `latency.ts`) with no network access at all. Its layout, its log
component and its panel structure are the starting point; **its numbers are
not.** Every value it renders today is invented, and the single largest failure
mode when replacing its mock with the real client is that a mocked constant
survives into the markup and is read as a measurement. §9 rule 1 and leg **d8**
exist for that specific transition: the first commit of the real demo deletes
`latency.ts` and every literal duration, and the columns start empty.

**Two lanes, one state machine.** The lane is a labelled toggle and every card
and log line carries it. **A replayed run never prints an unqualified timing.**

- **REPLAY** — a pinned static feed directory captured from Sepolia at a named
  manifest hash, containing the two transactions we made: the note at block
  **14,339,115** and the spend at **14,340,785**. Runs offline from a static
  directory, which is itself a claim worth demonstrating (no server, no API, no
  account). Timings are real; the *event* is recorded. A non-dismissible chip
  reads `REPLAY — recorded Sepolia history, discovery is live`.
- **LIVE** — a real feed (`strk20 run` over the Sepolia mirror by default; a
  mainnet feed URL is accepted). Real fetches, real folds, real chain events.

REPLAY is built first and is the CI target: deterministic, offline, and the only
lane in which the cold/warm comparison and the identity toggle are stable enough
to be watched. A demo that depends on a stranger owning 3 STRK and a
privacy-enabled wallet is a demo nobody sees.

**Layout, top to bottom: cards → stages → log.** This is the resolution of the
brief-versus-orchestrator tension, and naming the principle is what stops it
eroding: the brief's mutating last line is **the log**, which scrolls; the
orchestrator's side-by-side requirements are **cards**, which never scroll away.
They do not compete.

```
┌──────────────────────────────────────────────┬───────────────────────────────┐
│  A · COLD  |  WARM        (§5.1)             │  C · WHAT WENT TO THE NETWORK │
│  ┌───────────────┐  ┌───────────────┐        │  declared connect-src: <feed> │
│  │ COLD          │  │ WARM          │        │  ─────────────────────────────│
│  │ total    ___  │  │ total    ___  │        │  GET /feed/genesis.json  412 B│
│  │  fetch   ___  │  │  fetch   ___  │        │  GET /feed/manifest.json 47 kB│
│  │  inflate ___  │  │  inflate  ——  │        │  GET /feed/epochs/000000…     │
│  │  verify+fold  │  │  load    ___  │        │  … (grouped over 50; expands) │
│  │  discover ___ │  │  discover ___ │        │  ─────────────────────────────│
│  │ requests ___  │  │ requests ___  │        │  N requests · B bytes         │
│  │ bytes    ___  │  │ bytes    ___  │        │  module log sha256 3f9c…a71   │
│  └───────────────┘  └───────────────┘        │  A 3f9c…a71 / B 3f9c…a71  ✓   │
│  [ run cold ] [ run warm ] [ warm after reload ] [ A/B ]                      │
│  B · TRUST: verified=<grade> head l1 lastEpoch floor basis  [anchor rpc…]     │
├──────────────────────────────────────────────┤  scanner: key not found in 13 │
│  D · LOG (scrolls; last line mutates)        │  encodings · [ self-test ]    │
│  14:02:11 open      indexeddb persisted=false          12 ms                 │
│  14:02:17 fold      515 epochs verified and folded   5 812 ms                │
│  14:02:18 discover  1 note · 3.0 STRK · 0 spent      1 190 ms                │
│  14:02:22 ▸ waiting for the note…                        4.2 s               │
├──────────────────────────────────────────────┴───────────────────────────────┤
│  [ deposit ]  [ send ]  [ withdraw ]   subscription (●ON) [check now] [reset] │
└──────────────────────────────────────────────────────────────────────────────┘
```

Off to the side, one extra control: **`fetch full history`** (§8).

---

## 4. Stages — approve precedes swap

Each stage is a row of controls, dimmed until its precondition holds, **with the
precondition named in the dimmed state** rather than left to guessing. That is
the brief's mechanic, and it is what makes the log's one-pending-line rule
enforceable rather than aspirational.

**Stage 1 — Feed.** `[ run cold ] [ run warm ] [ warm after reload ] [ A/B ]`
No key exists yet, and the stage says so:

> No key is needed for this. The expensive part of this system runs with nothing
> about you in the process.

`run cold` calls `client.resetCache()`, then `close()` (terminating the worker
and freeing wasm linear memory, §4.11), then `indexedDB.deleteDatabase(name)`,
and only enters the cold state **after the deletion resolves** and a fresh
client exists. If deletion is blocked or storage is unavailable, the cold column
renders `unavailable — could not clear storage` and the run does not start: a
cold number that was not measured cold is worse than no number.

**Stage 2 — Identity.** Precondition: stage 1 has completed once.
`[ generate demo key ]  [ identity B ]` — and, in a local build only,
`[ paste viewing key ]`.

The §2 banner sits above these buttons and cannot be dismissed.

- A **generated** key discovers nothing (it is nobody's key), and the demo says
  so. It is useful precisely because it proves the request stream is identical to
  a real key's.
- **Paste is not enabled in the published build.** A page under our name that
  asks you to paste a wallet secret teaches exactly the behaviour that gets our
  users phished later. A viewing key is read-only, so this is not a spend risk —
  it is a habit risk, and the mitigation costs a build flag. The hosted build
  offers the generated key and the REPLAY identity; an operator demonstrating
  their own Sepolia note runs the local build.
- Where paste is enabled it accepts 64 hex, converts once to a `Uint8Array`,
  keeps it in a closure behind an `Account.viewingKey()` that returns a **fresh
  copy per call**, and never renders it.
- The UI shows the `keyId` (§4.4's HKDF id) and the address, plus one **computed**
  line: **`viewing key: held in this tab — 0 bytes of it have crossed the
  network`**. That line is not written prose: it is the §6.2 scanner predicate
  running live over every `RequestRecord` of the session.

**Stage 3 — Discovery.** Precondition: an identity exists.
`[ check now ]` and a toggle `subscription: ON | OFF`.
ON subscribes to `/feed/live` and runs discovery on every poke. OFF leaves
`check now` as the only trigger. Either way the log records how long it took and
**which trigger caused it** (`via sse` / `via poll` / `manual`) — merging them
would flatter the subscription. If `/feed/live` 404s or drops, the client
degrades to polling with a `status` event and the toggle renders `ON (polling —
this feed publishes no stream)`; a static-file mirror is a fully supported
deployment (§2.5) and the demo should demonstrate that rather than look broken.

**Stage 4 — Act.** Precondition: discovery has run at least once.
`[ deposit — do it in your wallet, we'll watch ]  [ send ]  [ withdraw ]`

The buttons must not look like they move funds. Each opens a **hand-off sheet**
headed with the §2 sentence and containing: what to do in your wallet or with the
SDK, the Sepolia pool address
`0x0254a6b2997ef52e9f830ce1f543f6b29768295e8d17e2267d672c552cfe0d91`, the exact
snippet, an `[ I've submitted it ]` button and an optional tx-hash field.
Pressing it arms a watcher (§5.4) and pushes the pending log line.

- **deposit** arms on a **new note id**.
- **send** and **withdraw** arm on the **nullifier of a selected note**, so the
  pending line resolves on the spend. That exercises exactly the property
  live-run §7 confirmed on real data: the nullifier our client predicted appeared
  verbatim in the chain's `NoteUsed` event. For **withdraw** the sheet carries
  one extra line, because it is where our value is clearest: the SDK cannot build
  a spend without knowing your notes, and that is what we just supplied,
  keylessly.

In REPLAY the buttons are **disabled with the reason shown** ("replay has no
chain to write to"), never silently faked. REPLAY's own equivalent is a control
that advances the demo's view of the pinned feed to just past the relevant
block, so discovery genuinely runs and genuinely finds the note.

---

## 5. State machine, log, and the pending line

### 5.1 States

```ts
type Lane = 'live' | 'replay';

type Feed =
  | { s: 'idle' }
  | { s: 'clearing' }                                   // deleteDatabase in flight
  | { s: 'syncing', kind: 'cold' | 'warm' | 'warm-reload',
                    phase: Phase, startedAt: number }
  | { s: 'ready',   feed: FeedState };

type Identity =
  | { s: 'none' }
  | { s: 'chosen', id: 'A' | 'B', address: `0x${string}`, keyId: string };

type Discovery =
  | { s: 'idle' }
  | { s: 'discovering', trigger: 'manual' | 'sse' | 'poll', startedAt: number }
  | { s: 'ready', notes: Note[], balances: Map<string, bigint> };

type Op =                                               // orthogonal region
  | { s: 'idle' }
  | { s: 'sheet', action: 'deposit' | 'send' | 'withdraw' }
  | { s: 'waiting', action: 'deposit' | 'send' | 'withdraw', armedAt: number,
      baseline: { noteIds: Set<string>, spent: Set<string> },
      target: { kind: 'note' } | { kind: 'nullifier', nullifier: string },
      pokes: number };

type Transport = 'live' | 'polling' | 'degraded';       // orthogonal region
```

Transitions worth pinning:

- `Feed.idle → clearing → syncing{cold}` only via the guard in §4 Stage 1.
  `syncing → ready` on the client call resolving; `phase` is driven by
  `{type:'progress'}` events, which is why §4.2 ships them.
- `Discovery.ready → Op.waiting` **arms a baseline**: the exact set of note ids
  and spent nullifiers at the moment of arming. Resolution is "a note id appears
  that is not in the baseline" (deposit, change note) or "a nullifier appears in
  `spent` that was not in the baseline" (send, withdraw). Comparing against a
  captured baseline rather than "the count went up" is what makes the elapsed
  number mean anything.
- `Op.waiting` resolves on the **first discovery pass that sees the change**,
  and the committed line records which trigger that pass had.
- **Exactly one `Op.waiting` at a time**; the stage buttons are disabled while
  one is armed. This is the reducer-level enforcement of the log's
  one-pending-line rule.
- `Transport` is independent and driven by `{type:'status'}` events; it renders
  as a dot next to the subscription toggle and writes one `warn` line per
  transition.

### 5.2 The log line model

```ts
type Provenance = 'measured' | 'recorded' | 'derived';

interface LogLine {
  seq: number;
  at: number;                    // performance.now() at creation
  lane: Lane;
  stage: 'feed' | 'identity' | 'discover' | 'await' | 'network' | 'error';
  text: string;                  // committed text
  pendingText?: string;          // shown while status === 'pending'
  status: 'pending' | 'ok' | 'warn' | 'fail';
  elapsedMs?: number;            // frozen at commit; ABSENT is legal (§5.4)
  metrics?: { label: string; value: string; provenance: Provenance }[];
  detail?: string;               // one line, revealed on click (error code, etc.)
}
```

Rules, enforced in the reducer rather than by discipline:

- **at most one `pending` line, and it is always the last.** A new operation
  while one is pending is refused by the stage gating, not queued.
- a pending line mutates in place and commits with its elapsed time when it
  resolves; **committed lines never mutate**;
- the pending line's live counter ticks off `requestAnimationFrame` at 100 ms
  granularity, but its **committed duration is computed from two
  `performance.now()` stamps**, never from the tick counter;
- `warn` for anything degraded but working (SSE dropped to polling; `verified`
  came back `server-asserted`; persistence fell back to memory); `fail` for a
  thrown `Strk20Error`, with `error.code` in `detail`;
- **nothing in the cards or the log may carry `provenance: 'derived'`.** Derived
  values exist only in the About panel, grey-italic, revealing their formula.

Rendering is text left, dot leaders, elapsed right-aligned:

```
[14:02:11] open       indexeddb, persisted=false ................... 12 ms
[14:02:17] fold       515 epochs verified and folded .............. 5.81 s
[14:02:18] discover   1 note · 3.0 STRK · 0 spent ................. 1.19 s
[14:02:22] ▸ waiting for the note…                                    ⠋ 4.2 s
[14:02:34] note 0xce526b28… 3.0 STRK · via sse .................... 12.6 s
```

### 5.3 What is logged, exactly

| stage | committed line | metrics attached |
|---|---|---|
| feed | `open — indexeddb, persisted=<bool>` or `memory (<reason>)` | `timing.phases.open` |
| feed | `cold start — folded N epochs to <head>` | total, fetch, inflate, verify+fold, requests, bytes, `verified` |
| feed | `warm start — restored from IndexedDB, N frames` | total, load, requests, bytes |
| feed | `feed unchanged at <head>` | total, requests (1 conditional head), bytes (0 on 304) |
| feed | `feed advanced <a> → <b>` | total, requests, bytes, `staleness` |
| feed | `snapshot applied at block B — S slots, verified=<grade>` | apply time |
| identity | `identity A — 0x04f2…9ab · keyId 7c1e… (key stayed in this tab)` | — |
| discover | `discovered N notes · X TOKEN · M spent · <trigger>` | discover total, engine ops, passes |
| discover | `+1 note 0xce52…7ff · 3.0 STRK @14,339,115` | time-to-discover (§5.5) |
| discover | `note 0xce52…7ff is now spent` | time-to-discover |
| await | pending `waiting for the note…` → `note 0xce52… found · via sse` | elapsed since arming, poke count while waiting |
| network | `N requests · B bytes · module log <sha256 prefix>` | — |
| network | `identity B produced the identical request log` | both hashes |
| reorg | `tail replaced — rewound to B` | — |
| error | `<code>: <message>` | `retryable` |

**Never logged**, and asserted by running `capture-scan` over a dump of the
demo's log state: the viewing key in any encoding, any channel key, any cursor,
or any truncated form of the key. The **address is** logged — it is the user's
own public address, displayed in their own browser — and is separately asserted
absent from every request record.

### 5.4 The pending line's deadline

`Op.waiting` has a **cancel** button, and it has an automatic commit at a
10-minute deadline. The commit reads

```
[14:12:22] gave up after 10:00 — no note seen ........................ warn
```

and it carries **no `elapsedMs` and no metric attributed to discovery**. This is
the deliberate composition of two correct objections (§0.5 S13): an eternal
spinner is every demo's failure mode, and a timeout that prints a duration
produces a number that looks like a measurement and is not one. Resolving on a
timer, a poll count, or anything other than an actual discovery result diffed
against the armed baseline is forbidden.

### 5.5 Two latency clocks, never merged

- **time-to-discover (ours)** — `armedAt` → the completion of the pass whose
  `added`/`spent` diff against the baseline contains the change. This is the
  number our product controls and the one the log line shows.
- **time-to-discover (end to end)** — `Date.now() − note.blockTimestamp * 1000`,
  shown separately and labelled *"includes block production and indexer lag —
  not our latency"*. It is never quoted as a product number.

---

## 6. The network panel

The panel's job is that a sceptic with five seconds can see there is no key and
no address in anything we send. It is fed entirely from `client.network()` —
the package's own record, emitted by its single fetch chokepoint (§4.10),
including requests issued inside the worker.

### 6.1 Header: the declared policy

The demo ships a CSP with `connect-src` limited to the feed origin (plus the
anchor-RPC origin when configured), `script-src 'self'`, no third-party fonts,
no analytics, no CDN. The panel renders it from the page's own meta tag, under a
label that says exactly what it is:

> **Declared policy** (read from this page's own CSP; confirm it yourself in
> devtools). The evidence is below.

That wording is the resolution of §0.5 S15: the CSP is a real, browser-enforced
boundary and is worth showing, but a page displaying its own meta tag proves
nothing to a sceptic, and this is the one panel that must be all evidence.

### 6.2 The evidence

1. **Every URL.** One row per `RequestRecord`, in order: method, **full URL never
   truncated in the middle** (truncation is where a query string would hide),
   status, bytes, ms, and `source` (`network` / `etag-304` / `idb-cache`). A
   `?`-free column and a `body: 0 B` column make the parameterlessness visible
   rather than asserted. Grouped after 50 rows (`epochs 000000–000514 · 515
   requests · 15.9 MB`) with a disclosure that expands to every row, because the
   claim is "you can read every URL" and a truncated list is not that claim.
2. **The SSE connection is a row**, not a hidden channel: `GET /feed/live
   (open)` with a live byte counter.
3. **The anchor RPC is visually separated** and annotated: *"your RPC, not the
   feed. Body: a public pool address and a public block number — identical for
   every user."* It is the only `POST` and the only non-feed origin.
4. **Totals**: `N requests · B bytes · module log sha256 <hash>`. The hash comes
   from `request_log_sha256()`, computed **inside the key-blind module** (§3.3),
   not from a UI-side list. That is the difference between "our UI says the lists
   match" and "the component that authors the URLs cannot see a key, and here is
   its own hash".
5. **The live key scan**, with its self-test. The `capture-scan` predicate runs
   in the page over every URL, every request header name and value, and every
   request body of the session, searching for the viewing key and the address in
   the same **13 encodings** the Rust scanner uses (minimal hex, padded, decimal,
   upper/lower, `0x`-prefixed, raw BE/LE bytes). Displayed as `key: 0 hits / 518
   requests`. A **self-test button** plants the key into a synthetic request
   record and shows the scanner catching it — a detector that has never fired
   proves nothing, and live-run §5 ran exactly this self-test. **The encodings
   list is imported from one shared fixture with the Rust scanner** so the two
   cannot drift.
6. **A key-presence indicator**, both halves live: `key in this tab: yes ·
   key in any request: no`.

### 6.3 Arithmetic, shown

Under the totals, the request count as arithmetic, because it is checkable by
eye against the live counter beside it:

```
mainnet:  1 genesis + 1 manifest + 515 epochs + 1 head = 518
sepolia:  1 genesis + 1 manifest + 606 epochs + 1 head = 609
with snapshots: 1 + 1 + 1 snapshot + 1 anchor + (0–1 epochs) + 1 head ≈ 5
```

The "with snapshots" line lives in the **About panel**, never in a live readout,
and is labelled as arithmetic rather than as a measurement. When the feed
actually publishes a snapshot the panel shows the real snapshot-lane request
count — measured, not projected — and the same arithmetic reads ≈ 5 on the same
card without editing a word.

---

## 7. The second-identity comparison

Two claims, both checkable in seconds, and they are different claims.

**(a) Discovery adds zero requests.** After a warm sync, `check now` under
identity A and then under identity B each add **zero** rows to the panel. This is
stronger and simpler than "identical", and it is the true statement: the
key-consuming code path has no network access at all — §3.9's import audit is
why. **The precision that keeps it honest on stage:** with `refresh: 'auto'` a
`getNotes` still issues a conditional head GET, so the A/B button runs with
`refresh: 'none'` and the caption reads *"discovery itself adds zero rows; the
feed pass that precedes it adds the same rows for everyone."* Stated loosely
this claim visibly fails in front of an audience.

**(b) Cold streams are identical.** `A/B` runs two full cold loads under two
identities and compares them. Four rules, each closing a different way the
comparison can lie (§0.5 S14):

1. **Both runs start from a deleted database**, in separate database-name
   suffixes, and the panel says why: *"a client's requests depend on what the
   feed has published and on what this browser has already stored — never on who
   you are. Both runs below start from an empty store, which is why they are
   comparable."* Comparing a cold run against a warm one compares two different
   questions.
2. **The manifest hash is pinned for the duration.** If the two runs saw
   different manifest hashes, the feed advanced mid-comparison and the panel says
   so and offers a re-run — **it never renders a verdict across two feed
   states.**
3. **The verdict is on `request_log_sha256()`**, the module's own hash. Same
   manifest, same starting mirror state ⇒ identical log; a difference is a red ✗
   with the diff shown, and there is no presentation in which a mismatch is
   hidden.
4. **Byte totals are reported separately**, because a `304` on one run and a
   `200` on the other legitimately changes bytes without changing the request
   sequence. The panel reads `bytes differ: identity B saw a head cut mid-run`
   rather than a red ✗ — an alarm that fires for a benign cause teaches viewers
   to ignore the indicator, which is worse than not having one.

One line of context sits underneath, marked as a recorded reference:
*"measured natively on 2026-08-31: 609 requests / 64,509 bytes on Sepolia, and
518 requests / 16 MB on mainnet — byte-identical across two different wallets,
before snapshots."*

---

## 8. The stage that shows a privacy rule costing something

One control, off to the side: **`fetch full history`**. It switches the client
to `coldStart: 'epochs'`, clears storage and re-runs, showing the request and
byte counts jump from the snapshot lane's handful to the full history's
hundreds. Its log line:

> full history requires the whole feed — fetching only the epochs containing
> your notes would make the request pattern a function of your notes

This is the only place the demo shows that we **pay** for the privacy rule
rather than merely claiming we would. A demo that shows only the cheap paths
invites the question of what we are hiding; this answers it before it is asked.

---

## 9. Measurement — what is shown, and how each number is obtained honestly

**Clock.** `performance.now()` only: inside the package (`SyncTiming`,
`NotesResult.elapsedMs`) and, for the waiting lines, in the demo around its own
arming. The module has no clock (§3.9), so every timing is taken in TypeScript at
a call boundary — exact at the boundary, approximate inside — and the panel
footnotes that.

| shown | obtained | included / excluded, printed in the UI |
|---|---|---|
| **cold total** | `performance.now()` around `sync()` + `getNotes()`, entered only after `deleteDatabase` resolves and a fresh client exists | includes fetch; **excludes** wasm instantiation on the very first load, reported separately as `boot` |
| **cold fetch** | wall time inside `fetch()`, summed | queueing behind other tabs' connections shows up here, and that is honest |
| **cold inflate** | span around `fzstd` only | excludes the sha256 that follows it, which is counted in verify+fold |
| **cold verify+fold** | `SyncTiming.phases.apply`, summed over `sync_supply` calls | excludes anything the wrapper did between Steps |
| **warm total** | `sync()` on a fresh client over the persisted DB, same session | the honest same-session comparator |
| **warm (after reload)** | a flag written to IDB before `location.reload()`; on boot, `performance.timeOrigin` → `sync()` resolving | includes wasm instantiation and IDB open — the number a returning user actually feels, and the one Safari's ITP can destroy |
| **warm load** | `SyncTiming.phases.load`, including reading frames from IndexedDB | — |
| **discover** | `discover_begin` … `discover_finish` | excludes the IndexedDB write of the sealed blob, counted in `export` |
| **time-to-discover** | §5.5 | two clocks, never merged |
| **requests / bytes** | counted from `RequestRecord`s; `bytes` is the received body length, not `Content-Length`; `transferBytes` shown alongside when `PerformanceResourceTiming` supplies it | an IDB hit is `source:'idb-cache'` and is **not** counted as a request; the panel shows `network N · cache M`. `transferSize` is 0 on cache hits and null cross-origin without `Timing-Allow-Origin`; the demo server sets that header and the UI prints `n/a` rather than a wrong 0 |
| **fold work** | `NotesResult.stats` (slots read, events scanned, passes) | a count, not a timing |

**The rules that keep it honest.** Each of these is a review gate on the demo
source, and together they are the difference between a demo and a staged one.

1. **No number is ever displayed that was not produced in this session.** A
   column that has not been run reads `not run yet`, never `0` and never a
   last-known value. If a measurement cannot be taken, the slot reads
   `unavailable` **and why** — no placeholder, no projection.
2. **A cold run is really cold — and the one thing it cannot clear is stated.**
   A page cannot clear the browser HTTP cache. The cold line therefore carries
   `(browser http cache may serve some artifacts)` and the panel's `source`
   column shows what actually came from the network. Understating this would be
   the easiest lie in the whole demo.
3. **Cold and warm are measured sequentially and displayed simultaneously.** Two
   folds racing on one core would make both numbers lies. Each column is stamped
   with its lane, its timestamp and its feed URL, and the card never averages
   across lanes or across feeds.
4. **Both columns show the same phase rows so the reader watches the subtraction
   happen.** Warm's `fetch` / `inflate` / `verify+fold` rows are **absent, not
   small**: they render struck through, carrying the byte count that was not
   downloaded.
5. **No median, no p95.** Single runs, honestly labelled. The statistical
   profile belongs to §4.6's throttled headless bench, and the demo links to its
   results rather than imitating them.
6. **Recorded reference numbers are grey, dated, sourced, and outside the live
   readouts.** The permitted set is exactly: 5.97 s cold fold / 0.03 s warm
   resync (native, full mainnet, 515 epochs, 16 MB feed, 60 MB mirror, 31 MB peak
   RSS); 118,960 events over 28,383 pool-active blocks; 139,131 writes over
   134,879 distinct slots; 31,077 notes in the anonymity set; 1.19 s to discover
   our own Sepolia note keylessly; 609 requests / 64,509 bytes byte-identical
   across two wallets on Sepolia and 518 requests / 16 MB on mainnet. Each links
   to its session in [live-run-findings.md](../research/live/live-run-findings.md),
   and the values are **parsed from that file at build time, never transcribed**
   (§10 leg d8). One line beside them says the browser's cold number will differ
   because 5.97 s is native and is the epochs lane, rather than inviting the
   comparison silently.
7. **Failures are logged in the same typeface as successes.** A
   `FEED_HASH_MISMATCH`, or a `verified` grade of `server-asserted`, appears in
   the log and in the trust card; nothing is swallowed. The grade is a
   three-state badge, never a green tick, and `server-asserted` renders with the
   CLI's own words: *"the snapshot's slot set is attested only by an anchor the
   feed itself published — set your own RPC for `anchored`."* Setting
   `anchorRpcUrl` in the UI flips it live, and that flip is the demo's best
   single argument.
8. **`download run log`** exports one JSON line per run carrying the full
   `SyncTiming`, the `NetworkSummary` including the module's request-log hash,
   the manifest hash, the chain id, the user agent and whether CPU throttling was
   detected. It is what makes a demo number reproducible instead of anecdotal,
   and it is the same record §4.6's bench consumes.

### 9.1 What the demo must never do

A checklist, because these are the failure modes that turn a real demo into a
staged one. It is a review gate on every change to `ts/demo`.

- Never display a constant where a measurement belongs.
- Never reuse a previous session's timing after a reload without labelling it.
- Never resolve a `waiting` line on a timer, a poll count, or anything other
  than a discovery result diffed against the armed baseline.
- Never hide a request from the panel — the SSE connection included.
- Never show a "with snapshots" number as if measured.
- Never let identity B share identity A's IndexedDB, which would make its "cold"
  run warm and its request list short.
- Never render an `IDENTICAL`/`DIFFERENT` verdict across two feed states.
- Never send the viewing key anywhere, including to the demo's own analytics.
  The demo has no analytics.
- **Never ship a path that submits a transaction.** The dev mode that would have
  submitted through the Privacy SDK using a funded Sepolia account key from a
  `VITE_`-prefixed variable is cut outright (§0.5 S16). `VITE_` variables are
  inlined into the client bundle **by design**, so "absent in the published
  build" is a policy and not a mechanism: one CI job with that variable set ships
  an account private key to every viewer of the page whose entire claim is that
  we never touch keys. It also could not work — the hosted mainnet prover URL is
  unpublished and not to be shared, and a self-hosted prover cannot shield,
  because the screening attestation is minted by the hosted prover's sidecar. CI
  asserts the published artifact contains no such identifier.
- Never ask for an account private key, build a transaction, or talk to a
  prover. The page and the README both say so.

---

## 10. Acceptance criteria — the legs that decide whether the demo is telling the truth

Playwright, in CI, against the **REPLAY** lane, so the suite is deterministic and
offline. Referenced from consumer-path.md §8.1.

- **d1 — cold then warm.** Both cards populate; warm total < cold total; the warm
  run's request list is exactly `{genesis.json, manifest.json, head.ndjson}`
  (§4.4's reload delta) plus SSE.
- **d2 — the identity comparison.** Two cold runs under two identities from two
  deleted databases: `request_log_sha256` equal, and the assertion is on the
  hash with the diff printed on failure. A run in which the two manifest hashes
  differ asserts that the UI rendered *"the feed advanced mid-comparison"* and
  **no verdict**.
- **d3 — discovery adds zero rows.** After a warm sync, `check now` with
  `refresh:'none'` under identity A and then B each add zero `RequestRecord`s.
- **d4 — the scanner is real.** The in-page scan reports 0 hits across the whole
  session; the self-test button makes it report 1; and the encodings fixture is
  asserted byte-identical to the one the Rust `capture-scan` compiles against.
- **d5 — the log's invariants.** Over a recorded reducer trace: at most one
  `pending` line ever exists, it is always last, and no committed line ever
  mutates.
- **d6 — a failure is a first-class outcome.** A corrupted epoch in a
  fault-injecting replay directory commits a `fail` line carrying
  `FEED_HASH_MISMATCH`, and the app remains usable afterwards.
- **d7 — the deadline commits without a number.** An armed op with no matching
  change commits `warn` at the deadline, and the committed line has **no
  `elapsedMs`** and no discovery metric.
- **d8 — recorded numbers are parsed, not transcribed.** Every grey reference
  value on the page equals the value parsed from `live-run-findings.md` at build
  time. One source; a stale quote fails the build rather than aging on screen.
- **d9 — the panel is complete.** `panel.rowCount === capture.requestCount`
  against the recording proxy's capture. A request that bypassed the chokepoint
  fails a leg instead of merely going unrendered — this is what makes the panel's
  completeness a fact rather than a hope.
- **d10 — the demo's clock measures what it claims.** Arm a waiting line against
  a synthetic note injected into the fixture feed at a known delay; assert the
  committed elapsed time equals that delay within tolerance. If only one leg
  survived, this would be it.
- **d11 — cold is cold.** With `deleteDatabase` stubbed to block, the cold column
  renders `unavailable — could not clear storage` and no cold number is produced.
- **d12 — no write path in the artifact.** The built bundle contains no
  `VITE_DEMO_*` identifier, no account-key input, and no prover or transaction
  submission call.

**REPLAY provenance is a shipping prerequisite, not a detail.** The pinned
Sepolia capture must be reproducible from the public feed at a named manifest
hash, with the reproducing command checked in beside it. Otherwise the demo
shows bytes nobody can re-derive, and its numbers mean nothing to anyone who did
not run it.

---

## 11. Conflicts resolved here

Summarised from consumer-path.md §0.5; the reasoning lives there.

| # | disagreement | ruling |
|---|---|---|
| S12 | three judges, three demo winners (cold/warm columns; network panel; cards-stages-log shell) | all three, in the arrangement each judge's own adoption list describes (§3) |
| S13 | timeout that commits with an elapsed time, vs cancel with no timeout | cancel **and** a deadline commit that carries no latency claim (§5.4) |
| S14 | how the A/B verdict handles benign differences | pinned manifest hash + deleted databases + module-computed hash + bytes reported separately (§7) |
| S15 | the page's own CSP as evidence | rendered, but labelled a declared policy; the evidence is the list, the comparison and the scanner (§6.1) |
| S16 | a dev mode that submits real transactions | cut, unanimously, with a CI assertion (§9.1) |
| S17 | "paste your viewing key" in the published build | local-build flag only (§4) |
