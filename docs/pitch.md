# Make persistence safe instead of forbidden

Every STRK20 wallet has the same problem. To find its own money it has to walk
pool contract storage with its viewing key, and that walk is expensive: about two
storage reads per note per user, every session. Upstream measured it —
**~2,250 reads and roughly a second at 1,125 notes** on a dedicated node, with a
ceiling of 7–9 requests per second per RPC node, and none of it cacheable because
every read is keyed to one user.

The reference answer is to do the walk on the server. The wallet uploads its
viewing key in every request body, the service decrypts the amounts, and the
service keeps nothing: it is stateless by design, with a hot cache and an indexer
written down as future work. So the wallet is told, in effect, **not to persist a
note registry** — re-derive it each session and pay the 2N reads again.

That advice is sound given the design. It is also the whole cost. Persistence is
forbidden because nothing in the pipeline makes a saved registry safe to trust:
no immutability boundary, no way to tell whether what you saved yesterday still
describes the chain today, no cheap way to check.

**Our claim is narrow and mechanical: we make persistence safe, and then it is
almost free.** A warm re-sync of the full mainnet pool history costs
**0.03 seconds**.

## What makes a saved registry safe

Four mechanisms, none of them clever, all of them load-bearing.

**Epochs are cut only below `l1_accepted`.** An epoch bundle covers a fixed block
range and is written only once that entire range is L1-final. A reorg therefore
cannot reach a published epoch — not "is unlikely to", cannot. Only
`head.ndjson`, the unfinalized tail, is ever rewritten, and it is small enough
(≤10k blocks, ~16 KB at today's volume) that a client refetches it instead of
storing it.

**Epochs are hash-chained and content-addressed.** The payload is canonical
NDJSON — a deterministic function of chain data — so two independent mirrors
produce byte-identical epoch files, and each names its predecessor's hash. An
omitted block is not a subtle difference; it is a fork you can see. The
acceptance suite runs two independent backfills and asserts the bytes match, and
rejects a tampered epoch by name.

**Reorg tombstones and a per-owner cursor that rewinds only above the floor.**
The one place a rewrite can happen is the tail, so that is the only place the
client has rewind logic. It rolls back above the last cut epoch and re-derives;
everything below the floor is untouched, because it cannot have changed.

**The mirror's own storage root is recomputed and checked against the chain.**
The server rebuilds the pool's Pedersen Merkle-Patricia root from its own slots
and compares it with `starknet_getStorageProof`. Because pool slots are
write-once — measured: **134,879 distinct slots across 139,131 writes, 96.9% of
writes are first writes** — a root match at block *B* attests every write at or
below *B*. One check covers all of history beneath it.

Put together, the persisted state is exactly the part of the feed that can never
go stale. That has a consequence worth saying out loud: **the browser client
needs no reorg logic at all.** It comes out simpler than the server, not more
complex.

## What it costs, measured

Full mainnet pool history, genesis 8,978,970 to head 14,128,517, on a laptop:

| | measured |
|---|---|
| feed size, full history | **16 MB** across **515 epochs** |
| events mirrored | **118,960** across 28,383 pool-active blocks |
| cold start over HTTP: fetch 16 MB, verify the whole hash chain, fold 515 epochs, run discovery | **5.97 s**, peak RSS 31 MB |
| cold start from a local directory, no HTTP at all | **6.18 s** |
| **warm re-sync** | **0.03 s** |
| client mirror on disk | 60 MB SQLite |

The two cold numbers being equal is the interesting one: the cost is the fold,
not the network. It also settles a design question in the direction we did not
want — at 6 seconds native, and WASM will be slower, a browser client cannot
re-fold history on every page load. The persisted mirror is not an optimization,
it is the thing that makes a browser client possible. That is why snapshots moved
up the roadmap from "nice cold-start win" to prerequisite.

Against the reference route, per wallet per session: ~2,250 chain reads and ~1 s
at 1,125 notes, uncacheable, versus **zero chain reads at query time** and 0.03 s
warm. The ingest cost is paid once, by the indexer, for everyone.

For scale of the thing being indexed, from `/v1/stats` over the full real
history: **31,077 notes** — the anonymity set — 2,628 registrations, 25,666
spends, 16,199 deposits across 31 tokens, 40,204 withdrawals across 34 tokens.

## The key never moves. Proven twice.

The reason persistence has to be made safe *here* rather than server-side is that
we do not have the key. The wallet fetches public static files —
`genesis.json`, `manifest.json`, a sequence of `epochs/…zst` — and does the
decryption locally with the upstream `discovery-core` engine. `Cargo.lock`
resolves that engine through a fork, which is packaging only: the fork's diff
against upstream is one `Cargo.toml`, its `src/` is byte-identical, and CI fails
the build if that stops being true.

In CI, a byte-scanner runs over a full proxy capture of a sync and asserts that no
encoding of the viewing key, the address, or any derived channel key crosses the
wire: minimal hex, 64-char padded, decimal, uppercase, raw big- and little-endian
bytes, base64. The same scanner demonstrably *does* find the key when it is
planted in a compat-mode body, so the negative is not vacuous.

Then we ran it on live Sepolia traffic through a recording proxy, with two
wallets: ours, holding a real key that finds a real note, and an unrelated one
that finds nothing.

| | result |
|---|---|
| viewing key in wallet A's traffic | **not found** in any of 13 encodings |
| address in wallet A's traffic | **not found** in any of 13 encodings |
| request streams, A vs B | **byte-identical** — 609 requests, 64,509 bytes each |
| detector self-test on a planted key | found it |

Byte-identical is the strong form. It is not that we do not log the key; it is
that the request stream carries no information about who is asking, so there is
nothing to log.

The full path, and which arrows the key is allowed on:
[docs/diagrams/dataflow.md](diagrams/dataflow.md).

## We minted a note and found it without the key

On Sepolia we made our own note — register, deposit 3 STRK, note creation, all in
one `apply_actions` at block 14,339,115 — and then pointed the indexer at it. It
found that note, and only that note, in **1.19 s**, deriving the note id and
amount from chain data alone. The indexer never saw the SDK's output.

A second transaction spent it in a private self-transfer. The nullifier our
client predicted appeared verbatim in the on-chain `NoteUsed` event: the
nullifier formula confirmed by the contract itself, not by our own test. Balances
counted only the unspent note. `strk20-sync verify` then proved both notes and
both spent-states against Starknet state roots via storage proofs, with the
indexer entirely out of the trust path.

The scripts that produced those two transactions ship in
[examples/sepolia](../examples/sepolia) so someone else can do it with their own
testnet account.

## What survived contact with reality

**A live contract upgrade, mid-run, that nobody announced.** While the Sepolia
server was running, the pool was upgraded on chain at block 14,339,893 to a class
we had never seen. `class_history` recorded it automatically, typed decoding went
to `degraded`, `/health` went `DEGRADED` with a warning naming the class — and
raw ingest and the feed continued uninterrupted. The keyless discovery above ran
against that very feed and still found our note, because discovery reads pool
storage, not decoded event types. A synthetic test asserting this is worth
something; the same thing happening unannounced, in production, during a run, is
worth more. Recovery was one flag.

**The root check earned its keep by failing.** The first time `verify-root` ran
successfully against mainnet it did not print OK — it reported a mismatch, and
the mismatch was real. Bisection with `verify-root --block` localized it to a
single block, and the cause turned out to be continuation-token paging across an
aggregating RPC endpoint: a token handed to a different backend node does not
error, it resumes somewhere else and silently drops the blocks in between. Two
paginated scans of the same range disagreed by 19 blocks with no error raised in
either. The fixture RPC in our own suite is a single honest process, so it cannot
express that failure at all — the only mechanism that could catch it was the root
check.

That same run proved something we had not been able to prove before: at the block
before the first dropped one, our mirror reproduces the chain's pool storage root
**exactly**, a Pedersen MPT root over ~100,000 real mainnet slots matching a proof
served by the chain. Root construction at scale was an open question. It is
answered.

## Where this goes next

The consumer path splits cleanly. Ingest stays on a backend. The consumer state
machine — fold the feed, run the upstream engine, emit notes and spent-state —
runs in two hosts: natively for self-hosters, and in the browser as a **WASM
module that is a pure computer**: bytes in, notes out, no network, no storage, no
async. Fetching, IndexedDB, zstd decompression and every `await` live in a
TypeScript wrapper. The spike is done: the upstream engine already builds for
`wasm32` unmodified — a two-line feature gate is not what unblocks the target,
it only sheds 24 crates (142 → 118) for consumers that opt out — and the module
is 231 KB gzipped.

Two APIs, deliberately. `KeylessClient` is the default and the thing we are
arguing for: the key stays in the browser. `DelegatedClient` talks to a server you
run, for SDK compatibility and self-hosters. The names describe the mechanic, not
a verdict.

## What is not true yet

Honesty is cheaper than a retraction.

- **The mainnet mirror is not currently complete.** The paging defect above cost
  139 blocks and 489 events in deep history; the repair is in flight, and until
  `verify-root` returns MATCH over the repaired range we are not claiming a
  verified mainnet mirror. The Sepolia backfill, by contrast, completed in a
  single process run with no aborts: 19,030 events across 4,455 pool-active
  blocks, 606 epochs.
- **No snapshot is published.** The code writes them; the publication gate
  requires an honest anchor, and on mainnet that gate is currently closed. The
  feed serves epochs and the tail, which is what the client needs.
- **Feed completeness is not provable to a consumer.** It is made auditable:
  content-addressed hash-chained epochs where an omission is a visible fork,
  server-side root verification, an append-only anchors log, and client-side
  storage-proof spot checks against your own RPC. The trustless fallback is
  self-hosting, and the whole thing is built to be self-hosted.
- **Compat mode receives viewing keys** by protocol definition. It is off by
  default, loudly labeled, and meant for your own machine.
- **Raw targeted mode leaks what direct RPC leaks**, including your address on
  the incoming path. Off by default, labeled.

Everything above with its raw evidence:
[docs/research/live/live-run-findings.md](research/live/live-run-findings.md),
[docs/research/live/sepolia-shield-run.md](research/live/sepolia-shield-run.md),
[docs/research/live/proof-window.md](research/live/proof-window.md),
[docs/research-answers.md](research-answers.md).
