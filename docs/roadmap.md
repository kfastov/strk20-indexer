# Roadmap — TypeScript consumer path

Written 2026-08-30. Scope: turn the working read path into something a TypeScript
web-app developer (or an agent writing one) can adopt without touching Rust.

## Architecture

Two blocks, one seam.

**Block A — ingest.** Node -> SQLite mirror -> feed (epochs, head, snapshots).
Backend only; never runs in a browser.

**Block B — consumer state machine.** Fold the feed into a local mirror, run the
unmodified upstream `discovery-core` over it, emit notes/balances/spent-state.
Runs in two hosts: the backend (keyed mode, for self-hosters) and the browser
(keyless mode, via WASM).

**The seam is `FeedTransport`** (`crates/client/src/transport.rs`), already in
place with `HttpTransport` and `DirTransport`. Block B does not know where bytes
come from. Two impls to add: in-process (server reads its own DB directly) and,
for the browser, none at all — TypeScript does the fetching and hands bytes to
WASM.

**Browser split.** WASM is a pure synchronous computer: bytes in, notes out. No
network, no storage, no async JS inside Rust. IndexedDB, SSE, caching and every
`await` live in the TypeScript wrapper. This is what keeps the engine's `Send`
bounds satisfiable and the module testable.

## Two APIs, deliberately

| | key | who computes | positioning |
|---|---|---|---|
| **Keyless** (default) | stays in the browser | client | the thing we sell |
| **Delegated** | goes to a server you run | server | self-host / SDK compat |

npm package: `KeylessClient` and `DelegatedClient` behind one interface
(`getNotes(key)`, `subscribe(key)`).

## Spike results (2026-08-30) — item 0, DONE

Run against upstream at the pinned tag `CONTRACT_V2_DEPLOYED_MAINNET_2026-07-08`
(rev `74841ca`, same rev as `Cargo.lock`).

- `starknet-providers` is declared in `discovery-core/Cargo.toml` but **used
  nowhere** in its `src` or `tests` at that rev. Making it optional behind a
  default-on `providers` feature is a two-line change — the shape of the
  upstream PR.
- With that gate, `cargo build -p discovery-core --no-default-features
  --target wasm32-unknown-unknown` **succeeds** (43 s cold).
- `strk20-feed` builds for `wasm32` with no features and with `mpt`. So MPT root
  verification — the snapshot anchor check — can run client-side.
- `strk20-feed` with `compress` does **not** build for wasm32 here: `zstd-sys`
  shells out to Apple clang, which has no wasm backend ("No available targets
  are compatible with triple wasm32-unknown-unknown"). Fixable with LLVM/wasi-sdk,
  but the cleaner answer is to decompress in TypeScript and hand raw NDJSON to
  the module.
- `crates/client` is **not** wasm-portable as written: `rusqlite` (bundled C
  SQLite) and `tokio::task::spawn_blocking` inside `ClientView`'s
  `RawStorageAccess` impl. The browser needs an in-memory view, not this one.
- End-to-end proof: a spike crate wrapping `sync_incoming_state` over
  `MockBackend`, built with `wasm-pack --target nodejs`, run from Node against
  `fixtures/upstream/devnet-state.json`:
  `alice: slots=48 complete=true notes=1 in 32 ms`, `bob: ... notes=0 in 16 ms`.
  Module size 427 KB raw, **231 KB gzip / 210 KB brotli**.

Not yet measured: fold time for a realistic mirror (full mainnet history) inside
the browser. That is the remaining sizing question.

## Plan

| # | What | Why | Size |
|---|---|---|---|
| 0 | ~~WASM spike~~ | done, see above | — |
| 1 | Snapshots in the cutter + storage-root anchor, verified client-side | cold start O(1) instead of replaying all history; the anchor keeps the trust story the hash chain gave us | M |
| 2 | SSE on the indexer: new head diffs, epoch-cut events | subscription instead of polling | M |
| 3 | Package Block B as a pure WASM computer (bytes in, notes out); in-memory view replacing `ClientView` | prerequisite for the browser client | M |
| 4 | npm `KeylessClient` + `DelegatedClient`, IndexedDB persistence, SSE, zstd in TS | the layer everything else exists for | L |
| 5 | `strk20-sync serve`: keyed HTTP + SSE on the client binary; third `FeedTransport` impl reading the DB in-process | the self-host surface `DelegatedClient` talks to | M |
| 6 | Sepolia config + test wallets/keys (pool `0x0254a6b2997ef52e9f830ce1f543f6b29768295e8d17e2267d672c552cfe0d91`) | debugging without mainnet funds | S |
| 7 | Upstream PR: feature-gate `starknet-providers` in `discovery-core` | removes our need for a patched fork | S |

Order: 1 -> 2 -> 3 -> 4, with 5/6/7 parallel.

## Deferred, with triggers

- **OHTTP** for the delegated/compat mode (hides who is asking). Not needed in
  keyless mode — every client fetches identical bytes. Trigger: delegated mode
  gets non-self-hosted users.
- **Prefix-bucket endpoint, then PIR.** See `docs/research/q9-pir.md`. Trigger:
  snapshot exceeds ~50 MB, i.e. roughly 8x10^5 records.
- **Write path in our binary.** Cut deliberately. Signing, key custody and a
  prover are exactly the surface this project exists to avoid. We are instead
  the read half of every write: the SDK cannot build a spend without knowing
  your notes, and that is what we supply keylessly — plus post-submit
  confirmation (nullifier landed, no reorg) through the subscription.

## Protocol facts that constrain the above

- Every pool write goes through `apply_actions`, which always validates a proof
  and collects the fee. There is no separate `deposit`/`register` entry point.
- Deposit screening: only deposits. `_apply_actions` returns a screening subject
  for `TransferFrom` and open-note deposits; for everything else the contract
  asserts `screening.is_none()`. The attestation is a SNIP-12
  `DepositorValidation{depositor, issued_at}` signed by FPI, max age 300 s.
- The hosted prover mints that attestation itself (its `proof-interceptor`
  sidecar). A self-hosted prover cannot, so **self-hosting can do everything
  except shield**.
- The proving service and the preflight RPC both receive the pool private key in
  `compile_actions` calldata. A hosted prover is therefore a permanent
  confidentiality dependency — which is why the write path, if it ever happens,
  belongs behind a self-hosted prover, and why shield stays in the user's wallet.
