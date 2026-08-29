# Q9 — Is PIR needed, and which model is viable? (STRETCH assessment)

**Context anchor.** Our keyless mode leaks which storage slots a client asks for (README "Honest limits": "the indexer still learns which slots a client asks for... Range mode hides even that at the cost of bandwidth"). Slots are Pedersen-hash-derived felts, computed client-side (`starknet-privacy/crates/discovery-core/src/privacy_pool/storage_slots.rs:1-60`, `views.rs:124-296`), so any PIR here is **keyword-PIR over a (felt-slot -> felt-value) KV store**: ~32B keys, 32B values (~64B/record stored), append-mostly, 10^4..10^7 records near-term. A wallet sync touches *many* slots (~2 reads/note per README), so batch cost, not single-query cost, is what matters.

## 1. PIR landscape, 2025–2026 (all single-server unless noted)

| Scheme | Offline (client) | Per-query server compute | Per-query comm | Append-mostly updates | Rust impl |
|---|---|---|---|---|---|
| **SimplePIR** (USENIX'23) | Hint ~√DB: **124MB per 1GB DB** (~724MB for 32GB) | 6.5–10 GB/s/core (linear scan) | ~250KB | Hint is linear in DB -> clients fetch hint *deltas* for changed rows (good fit) | reference is Go (ahenzinger/simplepir); small Rust ports exist (`simplepir` on docs.rs, si-co/simplepir) — research grade |
| **DoublePIR** (same paper) | Hint **~16MB, DB-size-independent** | 5.2 GB/s/core | ~350KB | same linear-hint delta trick | same repos |
| **FrodoPIR** (Brave, PETS'23) | Hint MB-scale per epoch | ~GB/s | KB-scale response | linear hint, delta-able | **brave-experiments/frodo-pir (Rust)** — research grade |
| **ChalametPIR** (CCS'24) = FrodoPIR + binary fuse filter -> **native keyword PIR** | FrodoPIR-style hint | ~100ms for 1M×256B KV | **response ~4KB** | **weak**: binary fuse filters are static — key inserts force full filter rebuild -> full hint re-download per epoch | **two Rust codebases**: reference claucece/chalamet; independent `chalamet_pir` / `chalametpir_client`+`chalametpir_server` crates on crates.io (v0.8.x, active 2025) |
| **Spiral** (S&P'22) | none (query-key upload ~MB once) | ~300MB/s/core | ~14KB up / ~20KB down | server re-encodes DB on change | **menonsamir/spiral-rs (Rust)**; Blyss built on it; research/startup grade |
| **Respire** (CCS'24) — built for **small records** | none material | 200–400MB/s (batch) | **6.1KB total for one 256B record from 1M-record DB**; batch 3.4–7.1× cheaper than Vectorized BatchPIR | server prep; **17× DB RAM (49–57× batched)** | **AMACB/respire (Rust)** — research grade, AVX2 |
| **YPIR** (USENIX'24) — "silent preprocessing" | **zero offline communication** | 12.1 GB/s/core | **2.5MB total per query @32GB DB** (sub-MB at our sizes) | **best fit**: no client hint -> updates free for clients | **Rust (USENIX artifact, menonsamir/ypir)** — research grade |
| **HintlessPIR** (Google, CRYPTO'24) | zero (hint computed homomorphically server-side) | ~GB/s | ~MB | paper explicitly targets "database updates frequently" case | **C++ only** (google/hintless_pir) |
| **InsPIRe** (eprint 2025/1352) | zero | high throughput | low (new LWE->RLWE packing) | good (silent) | none public — paper-stage |
| **Piano** (S&P'24) / **Plinko** (Eurocrypt'25) — client-preprocessing, sublinear online | client must **stream the entire DB** at setup | sublinear! | small | Piano: epoch rebuild = re-stream; Plinko: first with Õ(1) updates | Piano Go; Plinko spec/research repos only |
| **2-server IT-PIR (DPF/BGI)** | none | one AES pass over DB (μs–ms) | **~3KB/query** | trivial | thin Rust crates (`dpf-fss`, MatanHamilis/DPF); Google's production DPF lib is C++ |

**Keyword-PIR mapping.** Only ChalametPIR is natively keyword. Every index-PIR scheme can be lifted via hashing: since our slot keys are already pseudorandom (Pedersen), bucket records by key prefix into fixed-capacity padded buckets and PIR-fetch bucket `prefix(k)` — the classic Chor–Gilboa–Naor reduction, 1 query per lookup, no index map. A downloadable key->index map is a non-starter here: with 32B values the map is literally half the DB. Cuckoo-hashing (2–3 candidate indices) is the alternative when bucket padding is unattractive.

**Production-grade reality check (VERIFIED via repo survey): no PIR scheme has a *maintained, production-supported* Rust implementation in 2026.** Closest: `chalamet_pir` crates (active third-party maintainer), spiral-rs/ypir (author research code), frodo-pir (Brave experiments, quiet). SimplePIR-family is simple enough (~hundreds of lines of u32 matrix code) that reimplementing under audit is realistic — that simplicity is itself the strongest production argument.

## 2. The cheap alternative: bucketed/padded retrieval

Serve `GET /v2/slots?prefix=P&bits=k`. Pedersen slots are uniform, so buckets are balanced with no layout work.

- Cost: `N·64B / 2^k` per fetch. N=10^6, k=12 -> ~16KB; N=10^7, k=14 -> ~39KB.
- Leakage: k bits/query (anonymity set N/2^k records ≈ 610 at N=10^7,k=14). Repeated queries for the same slot always hit the same bucket (linkable but not compounding); a wallet's *set* of buckets across its notes is a fingerprint, and epoch-over-epoch intersection erodes the set — strictly weaker than PIR, strictly stronger than raw slot queries, and ~0 engineering cost on top of the planned range endpoint. Client-side k lets each user pick their own leakage/bandwidth point (k=0 degenerates to range mode = perfect).

## 3. Thresholds and verdict

Assume mobile wallet, 50MB tolerable (given).

- **Steady-state sync never needs PIR**: the private object is the per-epoch *delta*, and even 10^5 new records/day is 6.4MB/day in range mode — perfect privacy, trivial. (VERIFIED arithmetic; pool is days old, current N is closer to 10^2–10^4.)
- **Cold-start/backfill** is the binding constraint: full snapshot = N×64B ≤ 50MB up to **N ≈ 8×10^5 records**. At 10^7 (640MB) full download breaks; bucketed retrieval still costs only ~40KB/fetch at k=14 with ~600-record anonymity sets.
- PIR beats bucketing only when users need *cryptographic* (not k-bit) privacy for point lookups at N > ~10^6 — e.g. stateless payment-backend probes of specific nullifier slots, or recipient-channel lookups keyed by a user's public address (that key IS an identity, so it's the most privacy-sensitive lookup we serve).

**VERDICT: PIR is NOT justified now.** (VERIFIED premises, judgment call marked as such.) Current N is orders of magnitude below the threshold, range/delta mode already gives perfect privacy inside the 50MB budget, and no maintained production Rust implementation exists to lean on during a 2-day hackathon. Ship: (1) range mode, (2) prefix-bucket endpoint (near-free, halfway house), (3) an append-ordered snapshot/epoch layout so a PIR layer can bolt on without re-architecting.

**Trigger condition (put in README/roadmap):** when full-epoch snapshot > 50MB (≈ 8×10^5 records) *and* there is demand for better-than-bucket privacy on point lookups — adopt a **hintless single-server scheme (YPIR-family; InsPIRe when code lands)** over prefix-bucketed layout, because zero client hint matches an append-mostly DB with mobile clients; ChalametPIR is the fallback if epoch cadence is coarse (its static filter forces full hint refresh per epoch). Revisit **2-server DPF-PIR** only if multiple independent indexer operators emerge (federation breaks our single-operator self-hosting model, but it is 100–1000× cheaper than any single-server scheme).

Sources: YPIR eprint 2024/270 + USENIX'24 artifact; SimplePIR USENIX'23 (124MB/1GB hint, 16MB DoublePIR); Respire eprint 2024/1165 + AMACB/respire; ChalametPIR eprint 2024/092 + crates.io `chalamet_pir` 0.8; Plinko eprint 2024/318 (Eurocrypt'25); HintlessPIR Springer CRYPTO'24; InsPIRe eprint 2025/1352; brave-experiments/frodo-pir; facebookresearch/GPU-DPF.
