# STRK20 Open Note Indexer — research questions and product direction

**Project:** `kfastov/strk20-indexer`  
**Hackathon scope:** STRK20 Private Sprint, `IDEA-23 · Open note indexer`  
**Purpose of this document:** define the unanswered questions that determine the architecture and product trajectory before implementation is locked in.

> IDEA-23, verbatim: **“Note discovery today means scanning. A public, self-hostable indexer wallets can query without handing over a viewing key.”**

This document is intentionally not a final specification. It is a research backlog / decision document. Every P0 question should end with evidence from the current deployed STRK20 version, code references, and a concrete architecture decision.

---

## 0. Baseline: what is already known

### Current project hypothesis

The repository README currently proposes:

1. ingesting per-block storage diffs for the STRK20 pool into a persistent database;
2. a compatibility API that runs the reference discovery logic against the local index rather than live RPC;
3. a **keyless** API where the wallet keeps its viewing key locally, derives the storage slots it needs, and queries encrypted values;
4. an optional range/diff mode where a client downloads changes and filters locally;
5. WebSocket updates;
6. an explorer and operational metrics;
7. a CLI.

### Important facts from the current upstream STRK20 repository

These facts materially affect the project and should be treated as the starting point for research:

- The current reference discovery service is **stateless and RPC-backed**. It performs storage traversal, decryption and nullifier filtering on behalf of the wallet. It has no local database.
- The reference API receives the user's **private viewing key on each sync request**. It attempts to protect the key operationally (`SecretFelt`, zeroize-on-drop, redacted logs), but the discovery service necessarily sees the key and decrypted result while processing the request.
- The reference discovery architecture explicitly describes a future **hot indexed cache populated from per-block storage diffs**, with RPC fallback, as its recommended end state. That indexer/cache is still described as a future optimization in the current design docs.
- The reference service already supports **OHTTP** as an optional privacy layer. OHTTP can separate client IP metadata from request contents, but it does **not** stop the discovery service from seeing the viewing key/request content.
- The contract documentation currently lists events beyond deposits, withdrawals and viewing-key registration, including `NoteUsed`, `OpenNoteCreated`, `OpenNoteDeposited`, etc. Therefore the statement in this project's current README that the pool emits events *only* for deposits, withdrawals and key registration must be revalidated against the exact deployed pool version.
- The protocol contract is upgradeable and the official discovery design explicitly treats storage layout as versioned by block height.

### Immediate consequence

There are really **two different products** hiding inside the phrase “open note indexer”:

1. **Performance indexer / hot cache** — solves repeated RPC reads, rate limits, latency, backfill, reorgs and shared infrastructure. It can be fully compatible with the existing discovery service, but compatible mode still sees viewing keys.
2. **Privacy-preserving discovery transport** — lets a wallet perform discovery without giving any server its viewing key and ideally without revealing which exact note/storage slots it is interested in.

The strongest project should probably implement both layers, but they must be architecturally separated and evaluated independently.

---

# 1. P0 research questions — these determine the architecture

## Q1. What exact mainnet STRK20 version are we targeting?

**Question**

What is the exact pool address, class hash, release/tag, storage layout, event schema and discovery-core revision used by the live mainnet pool that the hackathon judges will interact with?

**Why this matters**

The upstream repository is evolving rapidly. README assumptions can become stale. An indexer built against `main` rather than the live contract can silently decode the wrong storage layout or infer nonexistent events.

**Research output required**

- [ ] mainnet pool address;
- [ ] deployed class hash;
- [ ] matching upstream release/tag/commit;
- [ ] deployment block;
- [ ] every upgrade block/class hash, if the pool has already been upgraded;
- [ ] event list for the deployed version;
- [ ] storage layout version(s);
- [ ] matching `discovery-core` and SDK revision.

**Decision produced**

A version table such as:

| block range | pool class hash | layout version | decoder |
|---|---|---|---|
| ... | ... | ... | ... |

No implementation should assume a single eternal layout.

---

## Q2. What does discovery actually have to discover locally?

**Question**

Starting with only the wallet's local private viewing key plus public chain/indexer data, can the client reproduce **all** reference discovery behaviour without sending the key anywhere?

This must be answered separately for:

- incoming channels;
- outgoing channels;
- subchannels;
- encrypted notes;
- note indices/cursors;
- nullifier derivation and spent-state checks;
- token discovery;
- viewing-key rotation/re-registration;
- sender/recipient filtering;
- preflight checks.

**Why this matters**

“Client computes the slots itself” is currently the central keyless hypothesis. It must be proven against real `discovery-core` traversal, not inferred from the protocol at a conceptual level.

**Research output required**

For each reference endpoint (`incoming_state`, `outgoing_state`, `preflight_check`):

- [ ] exact public inputs;
- [ ] exact secret inputs;
- [ ] every storage slot formula used;
- [ ] loop/traversal termination condition;
- [ ] which values must be decrypted before the next slot can be derived;
- [ ] which data can be derived entirely client-side;
- [ ] whether any step requires server-side knowledge that cannot be reconstructed from an encrypted state feed;
- [ ] test vectors showing local results exactly match `discovery-core`.

**Decision produced**

Either:

- **YES:** implement a standalone local discovery engine (`client-core`) and make keyless mode a first-class architecture; or
- **NO / PARTIAL:** state precisely which primitive is missing and design the smallest server-assisted protocol that does not reveal the viewing key.

---

## Q3. What is the minimal public dataset a keyless wallet needs?

Possible candidates:

1. arbitrary storage lookup: `slot -> value`;
2. all STRK20 pool storage diffs for a block range;
3. immutable compressed epoch bundles of pool diffs;
4. typed/indexed logical records (channel/note/nullifier) if they can be classified publicly;
5. some hybrid of the above.

**Key question**

Can the client perform complete discovery from **only append-only pool state diffs after its last cursor**, or does it periodically need arbitrary historical/current `getStorageAt` reads?

**Why this matters**

If append-only diffs are sufficient, the best architecture may be much simpler and more private than a database query API: produce a compact public feed that every wallet can consume identically and filter locally.

**Research output required**

- [ ] minimal fields per diff (`block`, `slot`, `new value`, perhaps old value, tx hash, etc.);
- [ ] whether initial sync requires a snapshot/current-state image;
- [ ] whether a wallet can resume from an epoch/block cursor without rescanning from deployment;
- [ ] whether nullifier/spent-state discovery works from deltas alone;
- [ ] whether channel discovery needs state that may have been written before the wallet's chosen sync start;
- [ ] whether a compact snapshot + subsequent diffs is sufficient.

---

## Q4. Why does a separate indexer help if a Starknet node already exposes state?

This was the central unresolved question in our discussion.

The answer must be demonstrated, not asserted.

Potential real advantages to validate:

- a raw node may expose whole-block state updates, while the indexer can publish **only STRK20 pool diffs**;
- an indexer can retain a cheap, queryable historical view without requiring every wallet to use an archive-capable RPC;
- it can compact and compress data into sync-optimized epochs;
- it can remove repeated `getStorageAt` probing across users;
- it can provide a stable cursor/reorg abstraction;
- it can provide snapshots and deterministic backfill;
- it can serve immutable data through ordinary HTTP/CDN/mirrors rather than requiring a high-quality Starknet RPC;
- it can provide push notifications/streams for services that need low-latency updates.

**Research output required**

Benchmark and protocol trace for the same wallet sync through:

1. `ContractDiscoveryProvider` / direct RPC;
2. current reference Discovery Service;
3. indexed compatible mode;
4. keyless targeted-slot mode;
5. keyless range/epoch mode.

Measure at least:

- number of Starknet RPC calls;
- upstream bytes;
- wallet bytes;
- p50/p95 latency;
- server CPU;
- DB reads;
- cold sync vs incremental sync;
- information leaked to the server.

If the proposed indexer cannot show a clear improvement in either privacy, latency, RPC load, reliability or developer ergonomics, that part of the architecture should be removed.

---

## Q5. Can state diffs be ingested reliably from ordinary Starknet RPC?

**Questions**

- Which RPC method/version gives the needed per-block state update?
- Does it include all privacy-pool storage writes and their new values?
- Can results be filtered server-side by contract, or must the indexer download the entire block's state diff and filter locally?
- How large are mainnet state updates in practice?
- What historical depth do common providers expose?
- Is an archive node required for backfill to pool deployment?
- Are there provider-specific truncation/rate-limit behaviours?
- Is a local Pathfinder node preferable for deterministic backfill?
- Does Apibara offer enough benefit to justify another dependency?

**Decision produced**

A concrete ingestion source and fallback hierarchy, e.g.:

`state updates RPC -> local persistent index -> getStorageAt fallback`

with explicit provider requirements.

---

## Q6. What exactly can be classified from a public storage diff?

The current README assumes the indexer can talk about “note, channel and nullifier counts”. This must be verified.

**Questions**

Given only `(storage_slot, value)` changes for the pool contract and no viewing key:

- can we tell that a write is a note?
- can we tell that it is a channel or subchannel?
- can we identify a nullifier write?
- can we associate a note with a token?
- can we distinguish encrypted-note creation from other opaque storage writes?
- can `NoteUsed` events reveal enough to count spends/nullifiers without compromising private associations?
- which logical types are public because of events/calldata, and which are cryptographically opaque because the storage address itself depends on secret-derived keys?

**Why this matters**

This determines whether the proposed explorer is technically honest. A privacy explorer that labels opaque state incorrectly is worse than having no explorer.

**Decision produced**

A table:

| Metric/entity | publicly derivable? | source | privacy caveat |
|---|---:|---|---|
| shields | yes/no | event/state | ... |
| unshields | yes/no | ... | ... |
| note creations | yes/no | ... | ... |
| nullifier uses | yes/no | ... | ... |
| anonymity set per token | yes/no | ... | ... |

Only metrics that survive this review should ship.

---

## Q7. What privacy does “keyless targeted slot lookup” actually provide?

**Known property**

It prevents the server from receiving the private viewing key and from directly decrypting note contents.

**Open questions**

- Does the set/order/timing of queried slots identify a user's channel, note sequence or activity history?
- Can the server link repeated syncs to the same wallet by stable slot patterns even without an account address?
- Are channel-list requests keyed by a public user address, thereby immediately identifying the user?
- Does requesting a nullifier slot tell the server which encrypted note the client is testing?
- How much more metadata is exposed than by direct RPC today?
- What changes when OHTTP is used (IP hidden, query contents still visible)?

**Required output**

A formal-ish leakage table for each API mode:

| Data | compatible | targeted keyless | bulk/epoch | PIR |
|---|---:|---:|---:|---:|
| viewing key | visible | hidden | hidden | hidden |
| decrypted amount | visible to service | hidden | hidden | hidden |
| exact queried slots | service derives/sees | visible | hidden among downloaded range | hidden |
| client IP | visible unless OHTTP | visible unless OHTTP | visible unless CDN/OHTTP | depends |
| sync timing | visible | visible | visible | visible |

This should become part of the public README/threat model.

---

## Q8. Is “download all pool diffs for an epoch and filter locally” practical?

This may be the most attractive privacy/performance trade-off for the hackathon.

**Proposed primitive**

The indexer emits immutable compressed bundles such as:

```text
/epochs/000123.zst
/epochs/000124.zst
```

Each contains only STRK20 pool storage changes for a fixed block range. Every wallet downloads the same artifact and filters/decrypts locally.

**Benefits if viable**

- no viewing key leaves the wallet;
- no exact storage-slot query pattern leaves the wallet;
- static data is cacheable by CDN and mirrors;
- indexer has no user-specific request processing;
- sync becomes deterministic and easy to self-host;
- it creates a clear reason to exist beyond a Starknet node: **compact, pool-only, history-preserving sync feed**.

**Questions to answer empirically**

- current bytes/block and bytes/day for STRK20-only diffs;
- compressed size with zstd/gzip;
- projected size at 10x / 100x current usage;
- initial sync size from deployment;
- CPU cost of local filtering/decryption;
- ideal epoch size (block count/time);
- how reorgs are represented before finality;
- whether fixed-size/padded epochs are useful to reduce timing/size metadata;
- whether the feed can be static and mirrorable after finality.

**Trajectory**

If this is cheap at realistic scale, ship it before sophisticated PIR. It is simpler, auditable and gives a very strong privacy story.

---

## Q9. Is PIR actually needed, and which PIR model is viable?

PIR should not be added merely because it sounds advanced.

Research only after Q8 establishes the data-size problem.

**Questions**

- single-server computational PIR vs multi-server information-theoretic PIR;
- database size and record shape;
- update frequency and preprocessing cost;
- query latency on realistic hardware;
- proof/cryptographic dependencies and auditability;
- whether a Rust implementation suitable for production exists;
- whether bucketed/padded retrieval gives almost all the benefit at far lower complexity;
- whether PIR hides the query but still leaks client identity/timing unless paired with OHTTP/relay.

**Decision**

PIR is a **stretch privacy backend**, not the core architecture, unless measurements show bulk epoch sync is already too expensive.

---

## Q10. How is note spent-state/nullifier discovery done keylessly?

The reference discovery engine derives the nullifier for each decrypted note and checks whether it exists, returning only unspent notes.

**Questions**

- exact nullifier derivation inputs;
- exact storage lookup/event used to test whether it has been spent;
- can a client update spent-state incrementally from the diff stream rather than querying each nullifier slot?
- can public `NoteUsed` events be used safely and efficiently?
- does the event expose the nullifier value directly in the deployed version?
- can a malicious/buggy indexer omit a spend and make the wallet temporarily believe a spent note is available?

**Decision produced**

A deterministic local state machine for:

`unknown -> discovered/unspent -> spent`

plus its trust assumptions.

---

## Q11. What is the trust model for a public indexer?

“Does not receive the viewing key” does not make the indexer trustless.

A public server could:

- omit a storage diff, making a wallet miss a received note;
- omit a nullifier/spend update;
- serve stale state;
- equivocate between clients;
- lie about the chain head;
- censor certain ranges.

**Questions**

- What can the client cheaply verify against Starknet block hashes/state roots?
- Is completeness of the diff stream cryptographically verifiable with existing Starknet RPC proofs?
- Should clients periodically compare against an independent RPC?
- Should the project support multiple mirrors and quorum/cross-checking?
- Can finalized epoch bundles be content-addressed and mirrored?
- What is the explicit failure mode: privacy-preserving but availability/truth delegated to operator, or independently verifiable?

This must be documented honestly.

---

## Q12. How should reorgs and finality work for each API mode?

Need separate semantics for:

- low-latency live stream (`ACCEPTED_ON_L2`);
- finalized immutable epoch feed;
- database snapshots;
- compatible discovery API cursors.

**Questions**

- rollback depth to retain;
- canonical block identity stored with every row;
- cursor invalidation semantics;
- how a wallet learns that already-consumed data was reorged;
- whether immutable epoch artifacts are produced only after finality;
- how live provisional data transitions into finalized bundles.

A good hackathon implementation can make this visibly robust rather than silently assuming no reorgs.

---

## Q13. How do contract upgrades/storage-layout versions affect historical indexing?

The official design explicitly requires block-height-based layout compatibility.

Research:

- proxy/replaceability mechanism used by the live pool;
- how to detect class replacement;
- whether slot derivation changes can be selected by class hash;
- whether old layout code remains necessary forever for historical backfill;
- how the API communicates its supported layout versions;
- behaviour when an unknown upgrade appears.

Recommended failure mode: **stop decoding typed data but continue retaining raw diffs**, rather than corrupting the index.

---

# 2. P1 research questions — product quality and competitive differentiation

## Q14. What should “compatible mode” actually be?

Potential implementation:

- reuse/implement the same storage backend interface used by `discovery-core`;
- serve the existing `/v1/sync/*` API from the local index/cache;
- preserve the SDK's current `IndexerDiscoveryProvider` behaviour.

Questions:

- Can `discovery-core` be consumed directly as a dependency cleanly?
- What trait/backend boundary exists today?
- Can Postgres/SQLite/RocksDB implement it without forking core logic?
- Does the official service's API version exactly match the SDK used by the live pool?

**Product positioning**

Compatible mode is valuable primarily for **self-hosting, performance and migration**, not as the privacy innovation. The README must say this explicitly.

---

## Q15. What should the keyless client library look like?

A server alone cannot solve keyless discovery. The secret-bearing part should be a reusable client library.

Candidate deliverables:

- Rust crate: `strk20-discovery-client`;
- optional WASM bindings for browsers/wallets;
- small TypeScript adapter implementing the official `DiscoveryProviderInterface`;
- deterministic test vectors against upstream `discovery-core`.

Questions:

- reuse upstream Rust crypto/slot functions vs reimplement;
- WASM compatibility of required crypto crates;
- private key zeroization in browser/native environments;
- how cursors/local wallet state are persisted;
- whether viewing-key rotation is handled.

If another hackathon project can import this library, that directly improves the project's ecosystem value.

---

## Q16. What should WebSocket/push mean without damaging privacy?

A user-specific subscription to exact slots may become a persistent fingerprint.

Possible safer designs:

- global stream of all pool diffs;
- stream per fixed public epoch/block range;
- token-agnostic global stream;
- user-specific targeted subscription only as an explicitly lower-privacy mode.

Research server and client bandwidth before choosing.

---

## Q17. Which database/storage engine is justified?

The current README says PostgreSQL, while the upstream future architecture speaks more generally about a hot read-optimized cache and its specs discuss SQLite in places.

Compare:

- PostgreSQL;
- SQLite for one-command self-hosting;
- RocksDB/Redb/LMDB-style embedded KV;
- hybrid immutable epoch files + small metadata DB.

Criteria:

- write amplification during backfill;
- lookup by raw Starknet storage slot;
- range/block queries;
- atomic reorg rollback;
- snapshot/export;
- operational simplicity;
- Docker quick start;
- memory/disk footprint.

Do not choose Postgres only because it sounds production-grade.

---

## Q18. What explorer metrics are both useful and privacy-safe?

Potentially useful public metrics:

- shield/unshield volume where amounts/tokens are publicly visible;
- pool TVL if derivable from public token balances;
- registrations;
- transaction counts;
- `NoteUsed` counts if publicly observable;
- indexer lag/health;
- sync feed size and anonymity-related aggregates.

Potentially dangerous/misleading metrics:

- “anonymity set per token” without a rigorous definition;
- note counts per token if token association is not public;
- channel counts if channel writes are not publicly classifiable;
- any visualization that accidentally makes timing correlation easier.

Deliverable should include a **privacy review of the explorer itself**.

---

## Q19. What are the success metrics?

A competitive infra project needs numbers.

Suggested benchmark page:

| Metric | Reference RPC discovery | Indexed compatible | Keyless targeted | Keyless epoch |
|---|---:|---:|---:|---:|
| viewing key sent to server | yes | yes | no | no |
| per-wallet RPC reads | ... | 0/hot | 0/hot | 0/hot |
| incremental sync latency | ... | ... | ... | ... |
| client bandwidth | ... | ... | ... | ... |
| server learns exact slots | yes/derivable | yes/derivable | yes | no |
| self-hostable | yes | yes | yes | yes |

Also benchmark:

- backfill duration;
- DB/index size;
- blocks/sec ingestion;
- recovery after restart;
- reorg rollback time;
- max lag under load.

---

## Q20. What mainnet demo proves the product rather than merely the library?

Hackathon scoring requires a live demo, a 3-minute demo video, and at least three successful mainnet transaction hashes touching the STRK20 pool in `strk20.json`.

An indexer is read-only, so the demo should deliberately create activity that it then discovers.

Suggested demo sequence:

1. Wallet A shields funds on mainnet.
2. A private transfer creates an encrypted note for Wallet B.
3. Wallet B syncs through the new **keyless** path while a network/request inspector visibly demonstrates that no viewing key was transmitted.
4. Wallet B spends/transfers/unshields, and the indexer updates spent-state incrementally.
5. Show the same sync through reference/compatible mode and compare privacy + RPC load.
6. Display the three mainnet tx hashes used for judging.

The demo should make the central value proposition observable in under one minute: **the wallet finds its private money, but the indexer never receives the viewing key.**

---

# 3. Recommended product shape

Subject to the P0 research above, the strongest architecture currently appears to be the following.

## Layer A — `indexerd`: generic public STRK20 state mirror

Responsibilities:

- follow the canonical STRK20 pool;
- ingest and persist pool storage diffs by block;
- maintain cursor/head/finality/reorg state;
- backfill from deployment;
- preserve raw diffs across layout versions;
- optionally expose current `slot -> value` state;
- generate compact finalized snapshots/epoch bundles;
- never receive or store viewing keys in its native keyless APIs.

This layer should be useful even to clients that do not trust your higher-level discovery code.

## Layer B — `client-core`: local private discovery

Responsibilities:

- hold the viewing key locally;
- derive channel/subchannel/note/nullifier slots;
- decrypt encrypted records locally;
- maintain local discovery cursor/state;
- filter spent notes;
- expose the same logical result shape expected by the Privacy SDK.

This is the component that actually fulfils the strongest reading of IDEA-23.

## Layer C — several transport/privacy modes (“privacy ladder”)

### Mode 1: Compatible

Reference `/v1/sync/*` semantics backed by the hot cache.

- Best compatibility.
- Fast.
- Viewing key visible to service.
- Recommended for self-hosted deployments/migration.

### Mode 2: Keyless targeted slots

Client derives exact slots and requests them.

- Viewing key hidden.
- Low bandwidth.
- Exact access pattern visible to indexer.

### Mode 3: Keyless bulk/epoch sync — **recommended headline mode if measurements support it**

Client downloads all STRK20 diffs for fixed epochs and filters locally.

- Viewing key hidden.
- Exact note/slot access pattern hidden.
- Static/cacheable/mirrorable.
- Higher bandwidth, likely very acceptable while STRK20 volume is small/moderate.

### Mode 4: PIR — stretch

Private retrieval for scale if bulk sync becomes too expensive.

- High innovation value.
- Only justified with realistic benchmark data.

This “privacy ladder” is a strong demo and documentation concept because it makes trade-offs explicit rather than marketing every mode as equally private.

---

# 4. High-value additions that would make the repository more competitive

## A. A real threat model

Add `docs/threat-model.md` with:

- adversary: indexer operator;
- adversary: RPC provider;
- passive network observer;
- malicious/stale indexer;
- what viewing key exposure reveals;
- query-pattern leakage;
- IP/timing leakage;
- OHTTP properties and non-properties;
- bulk sync properties;
- integrity/availability assumptions.

Privacy infrastructure is much more credible when it states precisely what it **does not** hide.

## B. A protocol/architecture document with sequence diagrams

Add `docs/architecture.md` covering:

- chain -> indexer ingestion;
- compatible discovery;
- targeted keyless discovery;
- bulk epoch discovery;
- reorg flow;
- initial snapshot + incremental sync;
- note discovery and nullifier update lifecycle.

## C. A side-by-side privacy/benchmark dashboard

The demo could have four columns showing reference, indexed-compatible, keyless-targeted and keyless-bulk.

Show live:

- request body (redacted only where genuinely secret);
- whether a viewing key crossed the network;
- RPC calls made;
- latency;
- bytes transferred;
- discovered notes.

This makes an infrastructure project visually understandable to judges.

## D. Immutable compressed “STRK20 sync feed”

If Q8 succeeds, this is potentially the project's most distinctive artifact.

Example public API/artifacts:

```text
GET /v2/head
GET /v2/epochs/:epoch
GET /v2/snapshot/latest
GET /v2/blocks/:from/:to/diffs
WS  /v2/live
```

Finalized epochs should ideally be deterministic, compressed, content-hashable and mirrorable.

## E. SDK drop-in adapter

Make adoption approximately:

```ts
const discoveryProvider = new KeylessIndexerDiscoveryProvider({
  baseUrl: "https://...",
  viewingKeyProvider,
});
```

The important point is that `viewingKeyProvider` is consumed **locally** and never serialized into the HTTP request.

A hackathon infrastructure project is much stronger if another app can adopt it in a few lines.

## F. Conformance tests against upstream

For fixed devnet/mainnet fixtures:

- reference discovery result == local keyless result;
- note IDs, channel derivations and nullifiers match Cairo/upstream test vectors;
- reorg rollback produces identical final state;
- viewing-key rotation works.

This reduces the risk of accidentally building a parallel, subtly incompatible protocol.

## G. One-command self-hosting

`docker compose up` should actually launch a useful system, not just exist in README.

Prefer a default profile that requires only:

- `STARKNET_RPC_URL`;
- pool address or known-mainnet preset.

Include health/lag/backfill status.

## H. Snapshot/bootstrap support

A new operator should not have to make millions of RPC calls every time.

Potential features:

- export/import snapshot;
- published finalized snapshot for the mainnet pool;
- resume from snapshot + verify chain identity;
- then apply incremental diffs.

This directly addresses self-hosting pain.

## I. Multi-mirror support

Because the data is public encrypted state, clients could fetch finalized epochs from any mirror/CDN/self-hosted instance. This can reduce both centralization and availability risk without sharing secrets.

A simple mirror protocol may be more useful than an elaborate explorer.

## J. Explicit compatibility matrix

Document:

| indexer version | pool class hash/layout | SDK | discovery-core |
|---|---|---|---|
| ... | ... | ... | ... |

This mirrors upstream's own compatibility discipline and makes the project look like infrastructure rather than a demo script.

---

# 5. Features to reconsider or de-prioritize unless research validates them

## Full transaction CLI (`shield`, `transfer`, `unshield`)

This duplicates the SDK and does not directly solve IDEA-23. A narrower infrastructure CLI may be higher value:

```text
strk20-indexer run
strk20-indexer backfill
strk20-indexer status
strk20-indexer snapshot export
strk20-indexer sync --keyless
strk20-indexer bench
```

Keep transaction commands only if they materially improve the mainnet demo.

## Fancy explorer

Useful only after Q6/Q18 establish which metrics are publicly and honestly derivable. A small “pool/indexer health + public activity” explorer is enough if deeper privacy metrics are speculative.

## PIR before measurement

Do not let PIR consume the core project before measuring bulk-diff bandwidth. A fixed-range download may already give stronger privacy with far less complexity.

## User-specific WebSocket subscriptions

They may create a persistent access-pattern leak. Prefer a global/block stream first.

---

# 6. Suggested repository structure

```text
strk20-indexer/
├── crates/
│   ├── indexer/             # chain ingestion + reorg/finality
│   ├── store/               # storage abstraction
│   ├── api/                 # raw/keyless/compatible HTTP APIs
│   ├── client-core/         # LOCAL slot derivation/decryption/discovery
│   ├── upstream-compat/     # reference discovery adapter
│   └── cli/
├── web/
│   └── demo/                # live demo + benchmark/privacy comparison
├── docs/
│   ├── research-open-questions.md
│   ├── architecture.md
│   ├── threat-model.md
│   ├── storage-layout.md
│   ├── api.md
│   ├── benchmarks.md
│   └── mainnet-demo.md
├── fixtures/
│   └── upstream-conformance/
├── docker-compose.yml
├── strk20.json
└── README.md
```

Exact crate boundaries can change; the important separation is **public indexing** vs **secret-bearing local discovery**.

---

# 7. Recommended research order

Do not investigate these in random order. The shortest path to a correct architecture is:

1. **Pin deployed mainnet version** — Q1.
2. **Trace `discovery-core` end-to-end and prove local reproducibility** — Q2, Q10.
3. **Determine minimal dataset required by that local algorithm** — Q3.
4. **Validate state-diff ingestion and measure real STRK20-only data volume** — Q5, Q8.
5. **Decide the headline privacy mode:** bulk epochs vs targeted slots vs hybrid — Q7, Q8.
6. **Define integrity/reorg/versioning model** — Q11–Q13.
7. **Only then finalize DB/API schemas.**
8. Build compatible mode and local keyless conformance tests in parallel.
9. Benchmark against the reference service — Q4, Q19.
10. Decide whether PIR is justified — Q9.
11. Build the smallest honest explorer — Q6, Q18.
12. Produce the mainnet transaction/demo evidence — Q20.

---

# 8. Definition of a strong hackathon submission

A strong final submission in the IDEA-23 scope would demonstrate all of the following:

- **Works on the live mainnet STRK20 pool.**
- **Three real mainnet pool transactions are listed in `strk20.json`.**
- **A wallet discovers its real encrypted notes without transmitting its private viewing key to the indexer.**
- **The result is proven equivalent to upstream discovery via conformance tests.**
- **The indexer materially reduces repeated RPC work and supports resume/backfill/reorgs.**
- **At least one access mode also hides exact note-slot access patterns** (ideally bulk/epoch sync; PIR only if justified).
- **Trade-offs are measured and documented rather than hand-waved.**
- **Another STRK20 project can integrate it with a small client adapter/library.**
- **Self-hosting is genuinely one command.**
- **The demo visually proves both privacy and performance.**

The core one-line pitch should remain extremely simple:

> **A fast, self-hostable STRK20 state index plus local discovery: wallets find their private notes without giving the indexer their viewing key.**

If the epoch/bulk approach proves practical, an even stronger version is:

> **STRK20 discovery as a public encrypted sync feed: every wallet downloads the same compact pool updates and discovers its notes locally, so the server learns neither the viewing key nor which notes the wallet is looking for.**

---

# 9. Sources to keep pinned during implementation

- Project repository: `https://github.com/kfastov/strk20-indexer`
- Hackathon / IDEA-23: `https://github.com/starkience/strk20-hackathon`
- Upstream protocol: `https://github.com/starkware-libs/starknet-privacy`
- Reference discovery service: `crates/discovery-service/`
- Discovery core: `crates/discovery-core/`
- Discovery service design specs: `crates/discovery-service/specs/`
- Privacy pool contract/source of truth: `packages/privacy/`
- SDK discovery providers: `sdk/`

**Research rule:** when docs and code disagree, pin the deployed mainnet class hash/version and treat that contract + matching release as the source of truth.
