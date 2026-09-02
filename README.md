# strk20-indexer

Open, self-hostable note indexer for the STRK20 privacy pool on Starknet, written in
Rust. A wallet discovers its own private notes without its viewing key, or its address,
ever reaching the server. The product is not a query API but a public verified sync feed:
content-addressed static files every wallet downloads identically and decrypts locally.

```
Starknet RPC ──► strk20 (server) ──► SQLite mirror ──► feed/ (static, content-addressed)
                                                          │
              wallet ──── GETs of public files only ──────┘
                └─ viewing key stays here; upstream discovery-core runs locally
```

## Why

To find its private money, a STRK20 wallet must walk pool contract storage with its
viewing key. The reference discovery service does that walk server-side: the wallet posts
its raw viewing key in every request body, so the operator can decrypt amounts, and each
sync costs about two RPC reads per note per user (upstream's measurement: ~2250 reads and
~1 s at 1125 notes, with a 7 to 9 req/s ceiling per node).

StarkWare runs that service: `discovery-service.alpha-mainnet.sw-dev.io` and
`transaction-prover.alpha-mainnet.sw-dev.io` both answer `/health` today. Whether it is
meant to be public for third parties is unanswered
([starkience/strk20-hackathon#121](https://github.com/starkience/strk20-hackathon/issues/121)).
The SDK's no-indexer fallback is reachable through an undocumented subpath, but
maturity-blind: it reports `created: 0` for every note, so the 10-block maturity rule
cannot be applied.

This project is the layer that takes the key out of the request.

## What it is

**`strk20`, the server.** Follows the pool with an events-first pipeline (pool-active
blocks are about 0.2% of all blocks), mirrors every pool storage diff and event to SQLite,
and cuts epoch bundles: zstd-compressed canonical NDJSON of pool diffs per fixed block
range, content-addressed, hash-chained, cut only below `l1_accepted` so they are immutable
by construction. Full mainnet history is 17 MB of epochs plus a 6.3 MB snapshot, growing
about 80 KB/day.

**`strk20-sync`, the client.** Downloads the feed, verifies the whole hash chain, folds it
into a local mirror, and runs the upstream `discovery-core` engine over it. It discovers
channels, notes and spent-state, resumes incrementally, and rewinds to the last L1-final
checkpoint on a reorg. The binaries share no secret-bearing code: the client does not link
the server crate, `SecretFelt` refuses serialization (compile-fail-tested), and the
transport trait has no method that could carry an address or a key.

**The browser path.** `crates/wasm` compiles the same engine to WebAssembly (892 KB raw,
411 KB gzip). `ts/strk20-discovery` wraps it as an npm package with two clients,
`KeylessClient` (feed only, key never leaves the page) and `DelegatedClient`. `ts/demo`
has three lanes: REPLAY (a recorded Sepolia capture, regenerated at build time), MAINNET (a
local feed) and LIVE (any running indexer). It measures cold and warm start in-page and
shows every URL it fetched. The npm package is not published yet.

**Modes, labeled.**

| Mode | What the server learns | Default |
|---|---|---|
| Feed (`/feed/*`) | that someone fetched public files; identical for every user | on |
| Raw targeted (`/v1/raw/*`) | which slots you query, so your address | off, `--enable-raw` |
| Compat (`/v1/sync/*`, `/v1/history`) | your raw viewing key, per request | off, `--enable-compat` |

Compat is wire-identical to the reference service for its four POST routes:
`crates/indexerd/src/compat/wire.rs` is upstream's `api/types.rs` at the pinned rev with an
8-line diff, all of it a header comment and two imports. With `INDEXER_URL` it is a drop-in
for key-holding backends; not yet for a browser SDK app, because those routes send no CORS
headers by design.

## What is proven

`cargo test --workspace` includes an acceptance suite
([crates/e2e-tests/tests/acceptance.rs](crates/e2e-tests/tests/acceptance.rs)) that spawns
the real binaries around a recording proxy and a synthetic RPC. It asserts that:

- keyless discovery output equals the upstream engine over upstream's own MockBackend,
  field for field, note creation blocks included;
- no encoding of the viewing key, the address, or any derived channel key crosses the
  wire, checked by a byte scanner that does find the key when pointed at a compat body,
  so the negative is not vacuous;
- two different wallets emit byte-identical request streams;
- a tampered epoch is rejected, and two independent backfills produce identical epochs;
- a mid-tail reorg is detected, rolled back, and the client rewinds without resyncing;
- an unknown-class upgrade degrades typed serving while raw ingest and the feed continue,
  and spent-state flips exactly the note whose nullifier lands on chain.


**On live Sepolia** ([sepolia-shield-run.md](docs/research/live/sepolia-shield-run.md),
[live-run-findings.md](docs/research/live/live-run-findings.md)) the same claims held
against a note we minted at block 14,339,115. It was found keylessly in 1.19 s. The
nullifier the client predicted appeared verbatim in the on-chain `NoteUsed` event. A
recording proxy found the key in none of 13 encodings, and two wallets' request streams
were byte-identical, 609 requests and 64,509 bytes each. An unannounced class upgrade at
14,339,893 was caught mid-run and the feed continued.

## Quick start

```bash
cargo build --release --workspace

# server, against mainnet
./target/release/strk20 run --db strk20.db --feed-dir feed --listen 127.0.0.1:8080

# client; the key stays here
echo 0x<viewing_key> > key.txt
./target/release/strk20-sync sync --feed http://127.0.0.1:8080/feed \
    --address 0x<your_address> --key-file key.txt --json

# browser demo (REPLAY lane needs no indexer)
cd ts && npm ci && npm run dev
```

`strk20 backfill` ingests to finality and exits. `status`, `epoch-verify`, `verify-root`,
`audit-coverage` and `enumerate-slots` audit; `rescan` and `recut-epochs` repair;
`mirror-pull` bootstraps from another instance. `strk20-sync verify --rpc <your-own-node>`
checks discovered notes against Starknet state roots, keeping the indexer out of the trust
path.

## Status

<!-- MAINNET-STATUS -->
Mainnet mirror as of 2026-09-02: complete and chain-verified, verify-root MATCH at block 14,260,184, 528 epochs cut, 13 anchors, one snapshot (epoch 1424, block 14,249,999).

<!-- HOSTED-STATUS -->
Hosted instance as of 2026-09-02: <https://strk20.nullref.cc> serves Sepolia and <https://strk20.nullref.cc/mainnet> serves mainnet, each exposing the same three paths (`/feed/*`, `/health`, `/v1/stats`) and each reporting OK with `verify_root_failed` false. For mainnet, pass `--feed https://strk20.nullref.cc/mainnet/feed` to `strk20-sync`.

## Honest limits

- **verify-root is three-valued.** On MATCH the server writes an anchor. On MISMATCH it
  latches `verify_root_failed` and stops publishing. On UNAVAILABLE, meaning no endpoint
  served a proof, epochs are still cut and published unverified. Anchoring is only as good
  as your RPC provider, and some serve no storage proofs at all.
- **The events-first ingest can miss blocks.** A block can write pool storage and emit no
  pool event, and `getEvents` cannot name such a block
  ([docs/spec/sound-ingest.md](docs/spec/sound-ingest.md) §1). The tail sweep that covers it
  is gated at 256 blocks, so a mirror further behind gets no sweep at all, which is exactly
  when the misses pile up. verify-root, not the sweep, is the backstop. Repair is
  `strk20 rescan` then `recut-epochs`.
- **Feed completeness is not provable to a consumer.** It is auditable instead:
  hash-chained content-addressed epochs make an omission a visible fork across mirrors, and
  the client spot-checks against your own RPC. The fallback is self-hosting.
- **Raw targeted mode leaks what direct RPC leaks**, including your address, and compat mode
  receives viewing keys by definition. Run either for yourself, not strangers.
- **The engine is consumed with zero source changes, but through a fork.** One commit
  against one `Cargo.toml`, making an unused dependency optional, pinned by rev in
  `[patch]`; CI fails if `discovery-core/src` differs from upstream by a byte. The upstream
  PR ([starknet-privacy#984](https://github.com/starkware-libs/starknet-privacy/pull/984))
  is open. See [docs/ops/fork.md](docs/ops/fork.md).

## Pool facts this build is pinned to

| | |
|---|---|
| Pool | `0x040337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a` (mainnet) |
| Deployed | block 8,978,970 (2026-04-20), class `0x30b8c540…4b4b30b` = `PRIVACY-0.14.2-RC.3` |
| Upgraded | block 11,632,886 (2026-07-09), class `0x67dddd89…76b554d` = `CONTRACT_V2_DEPLOYED_MAINNET_2026-07-08` |
| Decoding | one decoder covers all history: the 7 discovery events are identical in both classes, verified from both on-chain ABIs |

## Design documents

- [docs/spec/architecture.md](docs/spec/architecture.md), the spec
- [docs/spec/sound-ingest.md](docs/spec/sound-ingest.md), why events alone are not a sound index
- [docs/spec/consumer-path.md](docs/spec/consumer-path.md), the wasm and TypeScript surface
- [docs/research-answers.md](docs/research-answers.md), 20 research questions with on-chain evidence
- [docs/research/review/adversarial-review.md](docs/research/review/adversarial-review.md), the review that produced 22 fixes
- [docs/ops/hosting.md](docs/ops/hosting.md), running it

The working transcripts of the design process were removed from the tree on
2026-09-02 and live in git history in the commits before that one.

## License

Apache-2.0. Wire types and test fixtures vendored from
[starkware-libs/starknet-privacy](https://github.com/starkware-libs/starknet-privacy)
(Apache-2.0); per-file paths and hashes in
[fixtures/upstream/PROVENANCE.md](fixtures/upstream/PROVENANCE.md).
