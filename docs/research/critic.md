# Completeness Critic — STRK20 indexer research sprint

Key: `critic` · Date: 2026-08-29 · Deadline: 2026-08-31 23:59 UTC (~2.4 days)

Inputs reviewed: all files in `scratchpad/findings/` (q1, q2-q3-q10, q4-q19-q20, q5-q8, q6-q18, q7-q16, q9, q11-q12-q13, q14-q15, plus the three verifier reports). I did one piece of new evidence-gathering to settle the main cross-agent contradiction (see §1.1).

---

## 1. Contradictions between agents — status after adjudication

### 1.1 Live class → source mapping (RC.3 vs CONTRACT_V2 tag) — RECONCILED, no longer a contradiction

- `q1-version-pin`: live class 0x67dddd89…b554d = tag **PRIVACY-0.14.3-RC.3** (commit efc61cb), INFERRED-strong.
- `q6-q18-classifiability`: live class matches tag **CONTRACT_V2_DEPLOYED_MAINNET_2026-07-08** (commit 74841ca); the q2-q3 verifier noted 74841ca is *not a descendant* of the RC.3 release commit, making the two claims look inconsistent.
- **Adjudicated by me (VERIFIED locally, main clone `scratchpad/starknet-privacy`):**
  - `git diff PRIVACY-0.14.3-RC.3 CONTRACT_V2_DEPLOYED_MAINNET_2026-07-08 -- packages/privacy/src` → **only** `tests/test_e2e.cairo` (2+/141-). Tests are `#[cfg(test)]`; not part of the Sierra class. Deployable contract source is byte-identical between the two tags.
  - Same diff for `crates/discovery-core` → **empty**.
  - `git merge-base --is-ancestor` confirms RC.3 is NOT an ancestor of the V2 tag — they are parallel branches carrying identical contract + discovery-core source.
- **Resolution:** both agents described the same source. **Canonical pin: `CONTRACT_V2_DEPLOYED_MAINNET_2026-07-08` (74841ca)** — it is the organizer-named deployment tag (issue #195 corroborates "pool version 2.0"), and RC.3 is an equally valid alias for the contract/discovery-core. SDK differs slightly between the tags (pool-mode.ts exists on the V2 branch, deleted at RC.3+); for the published npm SDK use 0.14.3-rc.5 (ABI-compatible per q1). No downstream finding is invalidated by this fork-point subtlety.

### 1.2 Storage/event analysis source vs live class — MOSTLY reconciled, ONE real residual gap

- q6-q18 analyzed events/storage against the V2 tag **cross-checked against the live on-chain ABI** (live-pool-abi.json fetched) — sound.
- q7-q16 (leakage/slot math) verified slot derivation **only against RC.0 Cairo fixtures** and flags this itself as an open unknown. q2-q3 verified "no RC0→main changes to discovery storage vars" at source level, and §1.1 above extends that chain to the actual deployed source (discovery-core & contract identical RC.3↔V2-tag; RC0→RC5 storage untouched per q2-q3). So the *source-level* chain is now closed.
- **Residual (real) gap:** the Sierra ABI does not name storage vars, so nobody has yet confirmed a **live mainnet storage read at an RC-formula-derived slot address for the user-facing maps** (`public_key`, `recipient_channels`, `notes`, `nullifiers`). Only `auditor_public_key` (a singleton) was read live (q2-q3, q11). This is a 15-minute check once our own ViewingKeySet tx exists, and it is the single cheapest way to falsify the whole slot-addressing stack before building on it. Both q7-q16 and q14-q15 independently ask for it. → MUST-DO #3.

### 1.3 Minor numeric inconsistencies (flagged by verifiers; conclusions stand, but README/docs must use corrected numbers)

- Per-active-block pool entries: max is **≥68**, not 19 (verifier fetched blocks 9,439,303/9,439,322). Any buffer/row sizing should assume ≥68.
- Whole-chain raw volume: honest range **200–240 MB/day** (not the single 203 figure); zstd ~30 MB/day exact.
- Historical sliding-window peak: **~59×** current (~4.9 MB/day raw), not 43×/3.7 MB.
- `get_version()='2.0'` is **non-probative** for version discrimination (RC0 also returns '2.0'); the ABI diff is the proof. Don't cite get_version in the README.
- q6-q18 storage-var counts were wrong in the summary text (correct: 15 vars, 9 classifiable / 6 opaque); qualitative lists correct.
- Event/day figures differ across agents (q7-q16: ~530/day 7-day avg; q5-q8 lifetime avg ~900/day) — different windows, not a contradiction, but pick one convention in docs.

---

## 2. GAPS (unanswered or inference-only where verification was possible)

**G1. Q17 — DB choice: NOT ANSWERED by any agent.** No findings file addresses it. q4-q19-q20 silently assumes Postgres; q14-q15 defines the trait surface a backend must implement (slot-value-as-of-block reads, key-position-filtered events, BlockId resolution); q5-q8 gives volumes (~19 MB raw backfill, ~118k events, ~28k active blocks). The data says either SQLite or Postgres trivially works at current scale. Needs a 30-minute decision, not research. (Recommend: SQLite for the self-host one-binary story OR Postgres to match the docker-compose demo — decide and document; do not leave implicit.)

**G2. Live slot-formula verification for user-keyed maps** (§1.2) — inference-only today, verifiable in minutes after tx 1.

**G3. Epoch-bundle wire format / serialization** — q5-q8 recommends 10k/50k-block epochs and what to include (pool storage_diffs + replaced_classes + storage_root anchor) but no concrete schema; q7-q16 flags "exact wire framing not specified". Must be fixed by writing it (it's a design deliverable, not research).

**G4. WASM end-to-end with JS-backed RawStorageAccess** — cargo check passes on wasm32 (verified), but wasm-bindgen + async_trait Send-bounds untested. Known-risk small fix; demo plan's CLI decryptor path avoids it. Fine to leave as roadmap, but then the demo must not promise "browser" decryption unless the CLI fallback is acceptable in the video. (q20 script currently says "browser/CLI" — keep CLI as the committed path.)

**G5. Storage-proof verifier** — q11 captured a real proof (proof_lava.json) but nobody recomputed the Poseidon/Pedersen chain to the state root. INFERRED only. Out of 2-day scope; mark as roadmap in README, and don't claim "verified proofs" in the demo.

**G6. Second live RPC provider for cross-checks** — blast and nethermind free endpoints are dead; lava (0.8.1) + publicnode (0.10.2) are the two live ones. Spec-version skew (0.8.1 vs 0.10.2) between them was not tested for response-shape differences in getStateUpdate/getEvents (q5-q8 used both but did not diff shapes). Cheap smoke test during implementation. Also: proof retention window (25k–55k blocks on lava) unbounded precisely; storage proofs on publicnode work — good enough.

**G7. SDK/e2e client-side caching/cursor conventions not traced** (q2-q3 unknown) — matters only for keyed-compat mode (P2, first cut-line). Accept.

**G8. Sepolia identity of README hash 0x52107… and exact compiler build proof** — academic; class-hash→tag mapping is already over-determined by ABI + timing + §1.1. Accept as unknown; note in README that mainnet never ran 0x52107….

**G9. Four unmapped event selectors** — verifier resolved 2 (AppRoleAdminAdded/Removed); 2 remain (0x15e9615…, 0x3940d40…, 6 events total, roles-component-shaped). Irrelevant to notes; the unknown-selector counter (already designed) is the mitigation. Accept.

**G10. l1_accepted lag stability** — single 2.96 h sample. Only affects epoch-cut cadence; poll it live rather than assuming.

**Not gaps (settled, don't reopen):** keyless reproducibility (Q2), snapshot+diffs (Q3), nullifier/spend (Q10), one-decoder-covers-history (Q1: 7 note-flow events byte-identical across both classes), PIR deferral (Q9), reorg/finality design (Q12), upgrade watch (Q13), reuse-not-fork (Q14/Q15), push ranking (Q16), explorer metric ethics (Q18).

---

## 3. RISKS

### Architecture-breaking if wrong (each with current confidence + cheap falsifier)
- **R1. Slot-derivation formulas don't match the live class** (e.g. an unnoticed storage rename in the deployed build). Confidence high (source chain closed in §1.1) but the *only* live evidence is one singleton slot. Falsifier: G2 check after tx 1. If it fails, the keyless slab design still works (raw diffs are source-agnostic) but the client decryptor and classifiability split would need rework — do the check FIRST.
- **R2. Free archive RPC (lava/publicnode) drops depth or dies before the demo.** Both currently serve pool-genesis history (verified), but blast/nethermind died recently. Mitigation already designed (archive our own ingested raw data; content-addressed bundles); operationally: run the full backfill EARLY and keep the raw dump — after that, RPC loss only affects the live head.
- **R3. Pool upgrade mid-hackathon** — upgrade_delay=0, finalized=0, instant no-notice upgrades possible. Probability low over 2 days; mitigation (class-hash check per block, halt typed decode, keep archiving raw) must actually be implemented, not just designed.
- **R4. Reorg during demo window** — ACCEPTED_ON_L2 revocable en masse (Grinta precedent, ~2 h). If the P3 cut ("reorg auto-heal manual") is taken, the live demo could show phantom state after a reorg. Cheap guard: serve/cursor only ≤ l1_accepted for the *bundles*, label live-head data as provisional; keep (number,hash) cursors even in the MVP.
- **R5. Compat keyed-mode wire drift** — none found (frozen RC.0→RC.5), and it's the first cut-line anyway. Low.

### Operational risks for the 2-day deadline
- **R6. The 3 mainnet txs are the eligibility gate and depend on external UX** (Ready wallet + strk20.starknet.io/app; proving URL still unpublished for SDK route as of Aug 29 — issues #147/#221 open). Private transfer via the app may fail; fallback = 3× register/shield (verified to count: no `contracts` field ⇒ any pool-event tx passes). Needs ~25 STRK + a clean wallet (deposits are compliance-screened). **Every hour of delay compounds all other risks — do today.** Note: tx 3 (private transfer to a second registered wallet) is ALSO the test vector for G2/R1 and the demo's discovered note — triple-purpose.
- **R7. Demo-story overreach.** Several digest claims must be softened to survive judge scrutiny: whole-chain 200–240 MB/day (not 203); peak 59×; per-block ≥68 entries; browser decryption only if wasm lands (else CLI); "no key ever sent" is provable, "PIR-grade privacy" is not (keyless-targeted lookups still leak the address on the incoming path — q7-q16; the demo must use bulk/range mode to make the claim honestly).
- **R8. Backfill wall-clock unknown on our hardware** (~28k getStateUpdate calls; getEvents scan took ~5 min; sequential 0.4 s/call worst case ≈ 3 h). Start it night 1; cache raw responses so it never runs twice.
- **R9. Scoring hygiene misses**: strk20.json shape (NO `contracts` field — including it would make our app-made txs fail the `mine` check), demo URL resolution order (repo Website field most reliable), video URL required, hub refresh 30-min lag — leave ≥1 h buffer before 23:59 UTC. Posting the hosted URL in issues #121/#221 is the highest-leverage bonus but is a public action — get the user's explicit go-ahead.

---

## 4. MUST-DO before submission (ordered, 2-day-realistic)

1. **[Day 1, hours 0–3] Make the 3 mainnet txs** via Ready + strk20.starknet.io/app: register viewing key → shield ~15–20 STRK → private transfer to a second self-controlled registered wallet (fallback: extra shields). Record hashes. (Eligibility + test vectors; R6.)
2. **[Day 1] Pin the decoder source**: tag `CONTRACT_V2_DEPLOYED_MAINNET_2026-07-08` (74841ca) for blocks ≥ 11,632,886; `PRIVACY-0.14.2-RC.3` schema for 8,978,970–11,632,885; note in README that the 7 note-flow events are identical across both so one decoder suffices, and that upstream README's 0x52107… hash was never on mainnet. (§1.1 closes this — just write it down.)
3. **[Day 1, right after tx 1 confirms] Live slot smoke test (G2/R1)**: compute `public_key(our_addr)`, `enc_private_key(our_addr)`, `recipient_channels_base(recipient_addr)` via RC formulas, `starknet_getStorageAt` on mainnet, assert nonzero/expected; after tx 3, walk channel→subchannel→note→decrypt our own note with discovery-core. This is the go/no-go for the whole slot stack.
4. **[Day 1] Decide Q17 (DB)** in 30 min — SQLite or Postgres, justified by the measured volumes (~19 MB backfill, ~510 entries/day) and the self-host story; document the decision. (G1.)
5. **[Day 1 night] Ingest pipeline + full backfill**: getEvents active-block finder → getStateUpdate per active block → DB, from block 8,978,970; lava primary, publicnode fallback (smoke-test response-shape parity, G6); persist raw responses (R2/R8). Include from day one: class-hash check with halt-typed-decode-on-unknown (R3), (number,hash) cursors (R4), unknown-selector counter (G9).
6. **[Day 2] Keyless endpoint + client decryptor**: slab/nullifier-delta by block-range cursor; wrap discovery-core (native CLI committed path; wasm only if time, G4). Demo default must be bulk/range mode, not targeted lookups (R7 honesty). End-to-end: discover the tx-3 note.
7. **[Day 2] Freeze the epoch-bundle schema (G3)** minimally: NDJSON of pool storage_diffs + replaced_classes + storage_root anchor per 10k blocks, cut only ≤ l1_accepted; even a stub endpoint + doc section beats silence.
8. **[Day 2] Scoring gates**: strk20.json with 3 hashes + demo_video (NO `contracts` field), repo Website field = demo URL, README (what/why-privacy/run-locally/mainnet addresses; honest privacy framing per R7; corrected numbers per §1.3), license already present. Buffer ≥1 h before 23:59 UTC for the 30-min hub refresh (R9).
9. **[Day 2] Record the 3-min video early** (rough is fine); re-record only if time. Ask the user whether to post the hosted URL in issues #121/#221 (bonus scoring; public action requiring their approval).
10. **[If time] Headline benchmarks only**: B1 (backfill wall-clock), B3 (0 RPC reads/sync vs ~2N), B5 (slab fetch bytes/time), B8 (amortization). Cut everything else per the q20 cut-lines (keyed-compat API first).

Explicit non-goals for the deadline (document as roadmap, don't attempt): storage-proof verifier (G5), PIR/prefix-buckets (Q9 settled), WebSocket push, history/outgoing-sync endpoints, reorg auto-heal beyond L1-final cursoring.
