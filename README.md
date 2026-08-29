# strk20-indexer

Open, self-hostable note indexer for the STRK20 privacy pool on Starknet. Written in Rust.

**The core claim, mechanically proven in CI:** a wallet discovers its private notes without its viewing key — or even its address — ever reaching the server. The canonical product is not a query API but a **public verified sync feed**: content-addressed static files every wallet downloads identically and decrypts locally.

```
Starknet RPC ──► strk20 (server) ──► SQLite mirror ──► feed/ (static, content-addressed)
                                                          │
              wallet ──── GETs of public files only ──────┘
                └─ viewing key stays here; unmodified upstream discovery-core runs locally
```

## Why

To find its private money, a STRK20 wallet must walk pool contract storage with its viewing key. The reference discovery service does that walk server-side — the wallet **sends its private viewing key in every request body**, the operator decrypts amounts, and every sync costs ~2 RPC reads per note per user (upstream's own measurement: ~2250 reads and ~1 s per sync at 1125 notes, 7–9 req/s ceiling per RPC node). Upstream's design docs name a diff-fed hot cache as the intended end state and defer it. No public discovery endpoint exists; the SDK's no-indexer fallback is unexported and maturity-blind ([#121](https://github.com/starkience/strk20-hackathon/issues/121)).

This project is that missing layer, built keyless-first. The full research record behind every claim here — verified on the live mainnet pool and the upstream code — is in [docs/research-answers.md](docs/research-answers.md).

## What it is

**`strk20` (server).** Follows the pool with an events-first pipeline (pool-active blocks are ~0.2% of all blocks), mirrors every pool storage diff and event into SQLite, and cuts **epoch bundles**: zstd-compressed canonical NDJSON of pool diffs per fixed block range, content-addressed and hash-chained, cut only below `l1_accepted` so they are immutable by construction. At every cut it recomputes the pool's storage Merkle-Patricia root from its own mirror and refuses to publish if it disagrees with `starknet_getStorageProof` — a mirror that silently missed a write cannot ship epochs. Full mainnet history today is ~19 MB raw / ~6 MB compressed; ~80 KB/day of new data.

**`strk20-sync` (client).** Downloads the feed, verifies the whole hash chain, folds it into a local mirror, and runs the **unmodified upstream `discovery-core` engine** over it — same crypto, same traversal, pinned to the deployed contract's source tag and conformance-tested against upstream's own Cairo reference vectors. Discovers channels, notes, and spent-state; maintains a persistent registry with incremental resume and reorg rewind to the last L1-final checkpoint. The two binaries share no secret-bearing code: nothing in the client links the server crate, `SecretFelt` refuses serialization (compile-fail-tested), and the `FeedTransport` trait has no method that could carry an address or key.

**Modes, honestly labeled** (leakage analysis: [docs/research/q7-q16-leakage.md](docs/research/q7-q16-leakage.md)):

| Mode | Server learns | Default |
|---|---|---|
| **Feed** (`/feed/*`) | that *someone* fetched public files — requests are identical for every user (test-asserted) | ON |
| Raw targeted (`/v1/raw/*`) | which slots you query ⇒ your address on the incoming path; same leakage as direct RPC | off, `--enable-raw`, labeled header |
| Compat (`/v1/sync/*`, `/v1/history`) | your raw viewing key, per request — exact reference wire for SDK drop-in; run it only on your own box | off, `--enable-compat`, loud warning, bodies never logged |

**Explorer stats** (`/v1/stats`): only what a public observer can honestly derive — per-token shield/unshield volumes and TVL, global note count (= anonymity set), spend count, DeFi-interaction breakdown, registrations, upgrade history. No deposit↔withdrawal joins, no per-token claims about encrypted notes, no nullifier "linkage": those would be fabricated or harmful ([docs/research/q6-q18-classifiability.md](docs/research/q6-q18-classifiability.md)).

## Quick start

```bash
cargo build --release

# self-host the indexer against mainnet (defaults: lava RPC + publicnode fallback,
# the deployed pool address, verified class-hash decoder map)
./target/release/strk20 run --db strk20.db --feed-dir feed --listen 127.0.0.1:8080

# sync a wallet — the key never leaves your machine
echo 0x<viewing_key> > key.txt
./target/release/strk20-sync sync --feed http://127.0.0.1:8080/feed \
    --address 0x<your_address> --key-file key.txt --json
```

`strk20 backfill` ingests to finality and exits; `strk20 status`, `strk20 epoch-verify`, `strk20 verify-root` inspect and audit; `strk20 mirror-pull <url>` bootstraps a mirror from another instance with full chain verification. `strk20-sync verify --rpc <your-own-node>` checks every discovered note and its spent-state against Starknet state roots via storage proofs — the indexer is not in the trust path.

## What the tests prove

`cargo test --workspace` runs, among others, an **11-leg acceptance e2e** ([crates/e2e-tests/tests/acceptance.rs](crates/e2e-tests/tests/acceptance.rs)) that spawns the real binaries around a recording proxy and a synthetic Starknet RPC:

- keyless discovery output **equals the unmodified upstream engine over upstream's own MockBackend**, field for field, including note creation blocks;
- a **byte-scanner proves no encoding of the viewing key, address, or derived channel keys ever crossed the wire** (hex, padded, decimal, raw bytes, base64 — and the same scanner demonstrably *does* catch the key in a compat-mode body, so the negative is not vacuous);
- request streams for two different wallets are **byte-identical** — the feed is address-blind;
- a tampered epoch file is rejected by name; two independent backfills produce **byte-identical epochs**;
- a mid-tail **reorg** is detected, rolled back above the epoch floor, and the client rewinds to its L1-final checkpoint without resyncing from scratch;
- a surprise **contract upgrade** to an unknown class degrades typed serving while raw ingest and the feed continue (the real upgrade at block 11,632,886 happened with `upgrade_delay = 0`);
- spent-state flips exactly the note whose nullifier lands on-chain.

The MPT module is additionally verified against a **live mainnet storage proof** captured from the deployed pool, and the crypto/slot functions against the Cairo reference vectors shipped by the protocol itself.

## Design documents

- [docs/spec/architecture.md](docs/spec/architecture.md) — the full spec (three competing designs were drafted and judged; this is the synthesis)
- [docs/research-answers.md](docs/research-answers.md) — answers to all 20 research questions with on-chain evidence
- [docs/research/](docs/research/) — raw research reports, adversarial verifications, live pool ABI, measured volumes

## Deployed-pool facts this build is pinned to

| | |
|---|---|
| Pool | `0x040337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a` (mainnet) |
| Deployed | block 8,978,970 (2026-04-20), class `0x30b8c540…4b4b30b` = `PRIVACY-0.14.2-RC.3` |
| Upgraded | block 11,632,886 (2026-07-09), class `0x67dddd89…76b554d` = `CONTRACT_V2_DEPLOYED_MAINNET_2026-07-08` |
| Engine | `discovery-core` @ that tag (byte-identical across all published tags), consumed unmodified |
| One decoder covers all history | the 7 discovery events are identical in both deployed classes (verified from both on-chain ABIs) |

## Honest limits

- Feed completeness is not provable to a consumer; it is made *auditable*: content-addressed hash-chained epochs (an omission becomes a visible fork across mirrors), server-side verify-root at every cut, and client-side storage-proof spot checks against your own RPC. The trustless fallback is self-hosting.
- Raw targeted mode leaks exactly what direct RPC leaks — including your address on the incoming path. That is why it is off by default and labeled.
- Compat mode exists for SDK drop-in and receives viewing keys by protocol definition. Run it for yourself, not for strangers.

## License

Apache-2.0. Vendored reference wire types and test fixtures from [starkware-libs/starknet-privacy](https://github.com/starkware-libs/starknet-privacy) (Apache-2.0) carry provenance headers.
