# Findings: Q4 (cost model), Q19 (success metrics), Q20 (2-day mainnet demo) + hackathon-scoring intelligence

Key: `q4-q19-q20-value-demo` · Date: 2026-08-29 · Deadline: **2026-08-31 23:59 UTC (~2.4 days away)**

Local evidence roots (abbreviated below):
- `HACK` = `/private/tmp/claude-501/-Users-konstantinfastov-Projects-strk20-indexer/b9b259a5-132a-4a96-b7c3-68d3231f50a6/scratchpad/strk20-hackathon` (origin `starkience/strk20-hackathon`, HEAD `f3ec986` 2026-08-29 14:01 UTC — fresh today)
- `RC0` = `.../scratchpad/starknet-privacy-rc0` (tag PRIVACY-0.14.3-RC.0)

---

## 1. Submission requirements and scoring — VERIFIED from repo

### 1.1 What "submitted" means (all machine-checked, hub refreshes every 30 min)

| Requirement | Exact check | Evidence |
|---|---|---|
| Live demo URL | `requirements.demo = !!demoUrl`; resolved in order: `strk20.json:demo_url` → GitHub Pages → repo **Website** field → latest successful GitHub deployment | `HACK/scripts/build-projects.mjs:976-981`, `HACK/CONTRIBUTING.md:69-74` |
| Demo video | `requirements.video = !!entry.demo_video` — any non-empty URL in `strk20.json` | `build-projects.mjs:978` |
| Mainnet txs | `requirements.mainnet = verifiedTxs >= 3` (`MIN_MAINNET_TXS = 3`, `build-projects.mjs:605`); only first **10** hashes checked (`slice(0,10)`, line 649) | `build-projects.mjs:970-979` |
| Submission moment | "There is nothing to submit. Whatever your repository shows at **August 31, 23:59 UTC** is your entry." | `HACK/README.md:106`, `CONTRIBUTING.md:90` |
| README expectations | what it does + why privacy, how to run locally, mainnet addresses; docs = 15% of score | `CONTRIBUTING.md:98` |
| License | required by rules; counts to open-source score | `README.md:97`, `CONTRIBUTING.md:105` |

`strk20.json` at OUR repo root (never in the hackathon repo):
```json
{
  "transactions": ["0x…", "0x…", "0x…"],
  "demo_video": "https://youtu.be/…",
  "demo_url": "https://…"        // only if auto-detect fails; omit "contracts" (see 1.2)
}
```

### 1.2 Exactly how a transaction is verified — VERIFIED code semantics

`build-projects.mjs:645-690` (`verifyTransactions`), RPC = `https://rpc.starknet.lava.build` (line 700, env-overridable):

1. `starknet_getTransactionReceipt(hash)` must return a receipt (exists on **mainnet**).
2. `receipt.execution_status === "SUCCEEDED"`.
3. Some event in the receipt has `from_address == POOL` (`0x040337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a`, BigInt-compared, line 662).
4. **Only if** the project declared `contracts` in strk20.json: the tx must additionally emit an event from one of those addresses OR contain the address anywhere in calldata (lines 674-682). Otherwise `mine = null` and the tx **counts**: `verifiedTxs = txs.filter(t => t.ok && t.pool && t.mine !== false)` (line 970).

**Consequence (VERIFIED): a read-only indexer that deploys nothing should NOT list a `contracts` field.** Then any successful mainnet tx that emitted a pool event passes. Explicitly sanctioned: "A project that deploys nothing is judged on the pool alone: the sprint's privacy-wallet route is a real way to build" (`build-projects.mjs:641-643`); "Projects that deploy nothing of their own are judged on the pool alone. Hashes rather than an address because private transactions are relayed, so the on-chain sender is never you" (`CONTRIBUTING.md:58`).

**Can the 3 txs be made via the official app (strk20.starknet.io/app)?** VERIFIED yes, mechanically: the checker cannot and does not tie a tx to the submitting code when no contracts are declared. Normatively also yes: Day-0 doc itself says "use the app at strk20.starknet.io/app, which does registration and shielding through the UI" (`HACK/docs/MAINNET-DAY-0.md:60`) and only asks "List the hashes of calls **you actually made**" (`MAINNET-DAY-0.md:96`) — i.e. our own wallet's actions, made through any client. Nothing anywhere requires the transactions to be produced by our code.

### 1.3 Judging (human panel, announced Sept 4) — `HACK/README.md:118-129`

| Weight | Criterion | Our angle |
|---|---|---|
| 30% | STRK20 integration depth | note discovery/decryption pipeline IS the deepest part of the protocol; keyless mode is a direct answer to IDEA-23's "without handing over a viewing key" |
| 30% | Working mainnet product | hosted indexer synced to live pool + explorer UI, demo URL |
| 25% | Innovation | no competing IDEA-23 project exists (see §2); keyless discovery beats the reference design's key-upload model |
| 15% | Docs & open-source | README with self-host instructions, license (already present), API docs |
| bonus | "If another team depends on something you published, that counts in your favour" (`README.md:127`) | **Issue #221 literally asks the organizers for an `INDEXER_URL`; Day-0 doc says "discovery on the SDK route means a hosted indexer" (`MAINNET-DAY-0.md:37`). Publishing our hosted indexer URL + a note in issues #121/#221 before the deadline could make other teams consumers of our infra — the single highest-leverage scoring move available.** |

Hub "star" (AI assessment): currently **disabled** (`STAR_ENABLED = false`, `build-projects.mjs:760`) but the rubric is intelligence about what organizers value: "Cairo of its own …, a custom proving or nullifier scheme, **or an indexer is true**" for `complex` (`build-projects.mjs:785`). An indexer is on the organizers' short list of things that count as real engineering.

### 1.4 Registration — DONE (VERIFIED)

Our entry is merged: `registry.json` contains `kfastov/strk20-indexer`, category Infra, `inspired_by: IDEA-23`, and the hub row exists in `projects.json` (slug `strk20-indexer`, status `building`, 1 push, `verified_txs: 0`, requirements all false). No further registration steps. Remaining: push code, demo URL (set repo **Website** field — "the most reliable" per `CONTRIBUTING.md:73`), `strk20.json` with video + 3 hashes, before Aug 31 23:59 UTC. Hub re-reads every 30 min; last hub reindex today 14:01 UTC.

---

## 2. Competitive scan — VERIFIED from registry.json (174 entries) / projects.json (172 rows)

**No other project is building IDEA-23.** Ours is the only row with `inspired_by: IDEA-23` and the only one whose product is a note indexer for wallets.

Nearest neighbors (none competing on discovery-as-a-service):

| Project | Repo | What it is | Threat |
|---|---|---|---|
| hydra | github.com/charlesms1246/hydra | IDEA-24-shaped local dev stack: devnet + deployed pool + funded accounts + **local discovery service**, disclosure TUI. 0 verified txs. | Low — local devnet tooling, not a mainnet hosted indexer |
| tx404 | github.com/Abhyudday/tx404 | IDEA-26 shielding API/SDK "without touching a viewing key" | Low — write-path API, not discovery |
| cutout | github.com/dmetagame/cutout | signing guard w/ "supervised indexer + SQLite read model" (4 txs) | Low — indexes public evidence for pre-sign advice |
| himitsu-protocol | github.com/adipundir/himitsu-protocol | "public-edge indexer" for deposit monitoring (component) | Low |
| blindpay | github.com/raizo07/BlindPay | invoice indexer (component) | Low |
| starkwhisper | github.com/dino1x/starkwhisper | "fast indexing for transaction scanning" (component) | Low |
| shoal (iamdflame) | github.com/iamdflame/shoal | anonymity aggregation; authored the ContractDiscoveryProvider workaround in #121 | Low, but technically strongest adjacent team |

Field stats: 12/172 projects `finished` (all 3 requirements), 34/172 have ≥3 verified txs. Being submitted at all puts us ahead of >90% of the field on completeness.

---

## 3. Q4 — why an indexer beats raw-node discovery: the cost model

### 3.1 Upstream-measured facts (all VERIFIED from RC0 code/specs)

Per-item storage-read prices, `RC0/crates/discovery-core/src/discovery/mod.rs:17-45`:

| Operation | `getStorageAt` reads | Constant |
|---|---|---|
| channel count | 1 | `COST_NUM_CHANNELS` |
| per channel (fetch+decrypt inputs) | 3 | `COST_CHANNEL_INFO` |
| per subchannel | 2 (×2 with sentinel probe) | `COST_SUBCHANNEL_INFO` |
| per note (note + nullifier check) | **2** | `COST_NOTE` |
| note existence probe | 1 | `COST_NOTE_PROBING` |
| last-note-index bisect | `2·log₂(max)+1` | `last_note_index.rs:31` |
| public key | 1 | `COST_PUBLIC_KEY` |

So one full sync ≈ `1 + 3·C + ~4·S + bisects + 2·N` storage reads for C channels, S subchannels, N notes. Upstream capacity spec confirms the headline: "A discovery request can translate to up to roughly **2 × discovered_notes** storage reads … at 1125 notes ≈ **2250 reads/request**" (`RC0/crates/discovery-service/specs/19-scaling-and-capacity.md:29-31`).

Reference-service measured performance (spec 19, dedicated RPC node, batch=256):
- Single-user full sync latency **~0.93–1.03 s** (Juno/Pathfinder, 1125 notes, 1 page) — 19.3.
- Peak throughput **~7.0–8.9 req/s per RPC node**; the RPC node is the bottleneck (trie-lock / SQLite-WAL contention), not CPU/disk/network — 19.1, 19.8.
- "Cache effectiveness is limited for discovery workloads: each client queries keys specific to their account with little overlap" — 19.1. Per-user reads cannot be shared; only whole-state caching works.
- Statelessness: "The service is stateless, so the RPC node is the primary bottleneck" (19.1); hot cache + indexer are explicitly "future optimization / deferred" (`specs/04-proposed-architecture.md:21-32`, `specs/06-api-design.md:3`).
- Wallet guidance compounds it: "Don't persist the registry between sessions — rebuild with `discoverNotes` each time" (`HACK/docs/MAINNET-DAY-0.md:88`) → the full `2N`+ read bill is paid **every session, per user**.
- Key handling: "The service cannot discover … without access to decryption keys. Keys are provided per request. Responses contain decrypted data" (`specs/02-context-and-requirements.md:39-45`); `viewing_key` is a request field of `/v1/sync/incoming_state` (`specs/06-api-design.md:66-87`). TLS+zeroize mitigations exist (`specs/05`) but the trust model is "hand your viewing key to the server".

Measured by us today: public-RPC `getStorageAt` round trip on `rpc.starknet.lava.build` = **0.37–0.41 s per sequential call** (3 samples). A wallet doing raw-RPC discovery (ContractDiscoveryProvider-style) without batching pays that per read; even with JSON-RPC batching it pays the full read count per session.

Pool events exist for an ingest pipeline (VERIFIED `RC0/crates/discovery-core/src/privacy_pool/events.rs:37-78`): `Deposit`, `Withdrawal`, `EncNoteCreated` (keys `[selector, note_id]`, data `[packed_value]`), `OpenNoteDeposited`, `ViewingKeySet` — so an indexer can ingest via `starknet_getEvents` and/or per-block `starknet_getStateUpdate` storage diffs (the spec's preferred plane, `specs/01-summary.md`, `specs/13-alternative-data-sources.md`).

### 3.2 The claims table (what our indexer can say, and on what authority)

| # | Claim | Raw node / reference service | With our indexer | Status |
|---|---|---|---|---|
| 1 | Chain reads per user sync | ~`2×notes + 3×channels + probes` `getStorageAt` per user **per session** (measured upstream: 2250 reads @1125 notes) | **0 at query time.** Ingest cost is ~1 `getStateUpdate` (or 1 `getEvents` page) per block, paid **once, amortized over all users** | reads-per-sync: MEASURED upstream; amortization: OUR PROJECTION (standard indexer property) |
| 2 | Sync latency | ~1 s at 1125 notes on a **dedicated** RPC node; 0.4 s/read on public RPC without batching | single Postgres indexed query, target **<50 ms p50 / <200 ms p95** | upstream latency: MEASURED; our target: PROJECTION to be benchmarked (Q19) |
| 3 | Throughput ceiling | 7–9 discovery req/s **per RPC node** (node is the bottleneck) | DB-bound: thousands of reads/s on one Postgres; RPC load is O(blocks), independent of user count | upstream: MEASURED; ours: PROJECTION |
| 4 | Cost as users grow | O(users × notes) RPC reads per sync window | O(new blocks) ingest + O(result size) per query | PROJECTION (structural) |
| 5 | Viewing-key custody | key uploaded per request; server sees decrypted notes (spec 02/06) | **keyless mode**: serve encrypted channel/note slabs + nullifier set; wallet decrypts locally — server never sees a key | reference behavior: VERIFIED; our mode: DESIGN CLAIM to demo |
| 6 | Cacheability | per-account reads "little overlap between connections" → caches don't help (spec 19.1) | keyless slabs and per-epoch note feeds are user-independent → CDN/HTTP-cacheable; hot set served from memory | upstream: MEASURED claim; ours: PROJECTION |
| 7 | Cold start / re-sync | full re-scan each session (Day-0 guidance) | client cursor = (block, slab offset); delta download only | PROJECTION |
| 8 | Self-hosting | reference service exists but organizers ship no public endpoint; teams are asking for `INDEXER_URL` (issue #221) and the SDK's no-indexer fallback is un-exported (#121) | `docker compose up` self-hostable + our hosted instance | ecosystem gap: VERIFIED from issues; our deliverable: TO BUILD |

Honest framing for the README (protects the 30% integration-depth score per `CONTRIBUTING.md:104` "be precise about what is and isn't private"): the indexer does not reduce the *cryptographic* work (trial decryption is the wallet's either way); it eliminates the per-user **chain-read** bill and the **key-custody** requirement, and turns discovery from O(state) RPC probing into O(delta) feed consumption.

---

## 4. Q19 — benchmark table to fill during implementation (concrete definitions)

Methodology rules: fixed dataset = mainnet pool at a pinned block hash; report p50/p95 over ≥20 runs post-warmup; publish the script in-repo (`bench/` + one `make bench` target); compare like-for-like against (a) raw public RPC (lava) and (b) raw dedicated RPC if available. Upstream numbers quoted as "reference service (upstream spec 19)" — do not re-run their harness.

| Metric | Definition (what exactly to time/count) | How to measure | Baseline to beat |
|---|---|---|---|
| B1. Full backfill time | wall-clock from empty DB to head, pool deployment block → current head (~14.06 M head; start from pool deploy block) | indexer log timestamps; report blocks/s and total | n/a (report absolute; <1 h on a laptop is a strong headline) |
| B2. Ingest lag | p95 of (block timestamp seen on RPC head) − (block committed in DB), steady state over 1 h | Prometheus gauge or log diff | must be < block time ≈ 1.7 s·k for small k; target <5 s |
| B3. RPC reads per user sync | count of JSON-RPC calls issued to serve one wallet sync of an account with N notes | proxy counter in front of RPC client; N ∈ {10, 100, 1125} | reference: ~2N+ reads/session (spec 19.0.1); ours must be **0** at query time |
| B4. Sync latency (keyed mode, if implemented) | time from HTTP request to last byte, discovery of N-note account against our API | `hyperfine`/k6 against localhost + hosted; N ∈ {10,100,1125} | reference measured ~0.93–1.03 s @1125 notes; target ≤100 ms local |
| B5. Keyless slab fetch | time + bytes to download the encrypted-note slab & nullifier delta for a cursor K blocks behind head, K ∈ {1k, 100k, full} | curl -w timings; report bytes (gzip) | raw-RPC equivalent: K blocks × getStateUpdate ≈ 0.4 s each on public RPC |
| B6. Client-side trial-decryption throughput | notes/s a browser/Node wallet scans locally from the slab | bench in demo wallet code | none (new capability); report absolute |
| B7. Concurrent-user throughput | sustained syncs/s at 32 concurrent clients before p95 > 1 s | k6, 30 s window, fixed dataset | reference peak 7.0–8.9 req/s per RPC node (spec 19.3/19.6) |
| B8. Amortization headline | RPC reads/day: ours = blocks/day × calls/block; reference = syncs/day × reads/sync at U users × S sessions | arithmetic from B2/B3 counters | crossover plot: indexer wins for U ≥ 1 |
| B9. Reorg correctness | inject a synthetic 2-block reorg (or replay a real one) → DB converges, no phantom notes | integration test | reference: 409 BLOCK_REORGED + client re-sync |
| B10. Resource footprint | DB size (GB) and RSS after full backfill | `pg_database_size`, ps | report absolute (self-hostability claim) |

Minimum credible subset if time runs out: **B1, B3, B4/B5, B8** — those four are the value story.

---

## 5. Q20 — two-day mainnet demo plan

### 5.0 Blocker status (checked today via GitHub issues — all VERIFIED quotes)

- **Mainnet proving-service URL: still unpublished as of 2026-08-29.** Issues #121, #124, #135, #147, #158, #204, #221 all OPEN; zero maintainer replies supplying a URL. #147 latest (Aug 27): Facet "still need the three successful mainnet pool transactions… could the team provide temporary access". Self-hosting a prover works but is slow (Facet: "roughly 5–7 minutes per proof" on 2 vCPU). → **Do not plan any SDK-route private transfer that needs the proving URL.**
- **Every pool write needs a proof** (contra Day-0 §"needs no proof at all"): deployed class has only `__execute__`, `compile_and_panic`, `apply_actions` as non-admin entrypoints; proof-less `apply_actions` register reverts `EMPTY_PROOF_FACTS` (issue #147 comment, reproducible script in `ahmetenesdur/kese`; independently confirmed in #124). BUT this is invisible when using the **official app / a STRK20-enabled wallet**, which reach a prover themselves. Ready (ex-Argent) demonstrably works — issue #156 comment shows Ready's UI building private pool actions and a succeeded mainnet tx `0x04f38014…334ce` paying the pool fee. **Braavos does NOT implement the STRK20 wallet methods** (`wallet_strk20Balances → Not implemented`, probe in #121). → Use **Ready wallet + strk20.starknet.io/app**.
- **ContractDiscoveryProvider (SDK, no-indexer fallback): un-exported from the package root in 0.14.3-rc.5, but importable from the published `/testing` subpath**: `import { ContractDiscoveryProvider } from "@starkware-libs/starknet-privacy-sdk/testing"` (verified working against mainnet by kese, #147 comment; file:// workaround also posted in #121). Useful to us as a cross-check oracle for benchmarks, and proof that today's "official" alternatives are testing-grade — strengthens our pitch.
- Pool facts for the demo budget: **pool fee = 6 STRK per private transaction** (VERIFIED live: `get_fee_amount` → `0x53444835ec580000` = 6e18 FRI; charged per tx from shielded balance, per #156). Block time ≈ **1.70 s** (measured over ≥200k blocks, #121 thread). Live pool is **V2** (`class 0x67dddd89…b554d`, "pool version 2.0", lineage `CONTRACT_V2_DEPLOYED_MAINNET_2026-07-08`, issue #195) — matches our measured class hash; RC0 README's class is stale, so index against the deployed V2 class, and sanity-check storage layout against it, not RC.0 docs.

### 5.1 (a) Minimal eligibility — 3 mainnet txs, no proving URL needed

All via **Ready wallet (Mainnet) + strk20.starknet.io/app**, from our own wallet; hashes → `strk20.json`. Do this FIRST (today), it de-risks everything:

1. **Register viewing key** — emits `ViewingKeySet` from the pool → passes the pool-event check. Gas only.
2. **Shield STRK** — emits `Deposit`. Shield ~15–20 STRK (covers the 6 STRK pool fee for tx 3 + transferred amount + slack). Gas + amount. Deposits are compliance-screened; use a clean wallet.
3. **Private note-to-note transfer** (app/Ready UI, e.g. to a second wallet we control that also registered) — the tx the whole demo hangs on: emits only encrypted note + nullifier. Costs 6 STRK pool fee from the shielded balance.
   - **Fallback if the transfer UI fails:** a second shield (different token or amount) or an unshield attempt via the app. Any successful pool-event tx counts — verification does not distinguish tx types. 3× register/shield alone is fully eligible.

Rules check (VERIFIED §1.2): no `contracts` field in our strk20.json → `mine = null` → app-made txs count. They are "calls you actually made". Budget: ~25 STRK + gas total.

### 5.2 (b) Demo sequence proving keyless discovery (3-min video + live demo URL)

Story: "Our wallet received a private transfer. No viewing key ever left the browser."

1. Open explorer UI (hosted demo URL): pool stats — blocks indexed, ingest lag, deposits, `EncNoteCreated` count, nullifiers. Show OUR three txs in the explorer (ties eligibility to product).
2. Point the demo wallet page/CLI at the indexer in **keyless mode**: it downloads the encrypted channel/note slab + nullifier delta; derive the viewing key locally (standard `signMessage`, Day-0 §Step 1); trial-decrypt **in the browser/CLI**; the private note from tx 3 appears with amount + sender — while the network tab / server log shows no key material ever sent.
3. Kill switch contrast (10 s): the reference design's request body literally contains `"viewing_key"` (show spec/06 excerpt); ours doesn't have the field.
4. Amortization slide: B3/B8 numbers — "reference: ~2 reads per note per user per session (~2250 @1125 notes, ~1 s on a dedicated node); ours: 0 chain reads per sync, one ingest per block for everyone".
5. Close: `docker compose up` self-host one-liner + hosted URL other teams can use (post it in issues #121/#221 → "another team depends on what you published" scoring bonus, README.md:127).

### 5.3 Cut-lines (in order of what to drop as time runs out)

| Priority | Item | Cut? |
|---|---|---|
| P0 | 3 mainnet txs via app/Ready (today) | never — eligibility |
| P0 | Ingest pipeline → Postgres (events-first: `getEvents` for Deposit/EncNoteCreated/ViewingKeySet/Withdrawal/Nullifier data via state diffs); backfill from pool deploy block | never — the product |
| P0 | Keyless endpoint: encrypted slab + nullifier set by block-range cursor; wallet-side decryptor (CLI is enough) | never — the differentiator & IDEA-23's literal ask |
| P0 | strk20.json (txs+video), demo URL reachable, README + license | never — scoring gates |
| P1 | Minimal explorer web UI (stats + tx list) | degrade to a JSON status endpoint rendered by a static page |
| P1 | 3-min video (screen-record the CLI flow; record early, re-record only if time) | never cut, but can be rough |
| P2 | Reference-compatible `/v1/sync/incoming_state` (keyed mode) for SDK drop-in (`IndexerDiscoveryProvider` compatibility) | cut first if behind; claim "planned", keep the route stubbed |
| P2 | Benchmarks beyond B1/B3/B5 headline numbers | cut to the 3 headline numbers |
| P2 | WebSocket live updates | cut; poll |
| P3 | History endpoint, outgoing-channel sync, reorg auto-heal (manual re-backfill is acceptable at demo scale) | cut; document as roadmap |

Biggest schedule risks: (1) V2 storage-layout drift vs RC.0 code we're porting from — mitigate by validating one known note (our own tx 3) end-to-end first; (2) app/Ready private-transfer friction — mitigate by doing the txs TODAY with the 3×shield fallback; (3) Postgres backfill time unknown — mitigate by starting backfill from the pool deploy block, not genesis, and caching raw responses.

---

## 6. Answers in one line each

- **Q4:** The reference route pays ~2 `getStorageAt` reads per note per user per session (measured upstream: 2250 reads & ~1 s at 1125 notes; 7–9 req/s ceiling per RPC node) and requires uploading the viewing key per request; an indexer pays O(1 RPC call per block) once for all users, serves syncs from Postgres in ms, makes keyless (CDN-cacheable) discovery possible — the one thing upstream measured as impossible to cache per-user.
- **Q19:** Fill the B1–B10 table (§4); minimum credible: backfill time, RPC-reads-per-sync (0 vs 2N), sync latency vs upstream's ~1 s, and the amortization crossover.
- **Q20:** Do the 3 txs today via Ready + strk20.starknet.io/app (register + shield + private transfer; ~25 STRK; no proving URL needed — that blocker is real and still unresolved as of Aug 29 but only bites SDK-route writers); demo = explorer + browser-side trial decryption with provably no key egress; cut keyed-compat API, WS, and history first.
