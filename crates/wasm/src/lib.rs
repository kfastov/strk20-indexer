//! `strk20-engine` — Block B in the browser, as a **pure synchronous
//! computer**: feed bytes in, notes out.
//!
//! This crate is a `wasm-bindgen` facade and nothing else. Every line of state
//! machine — feed verification, the epoch hash chain, the snapshot ladder, the
//! reorg discipline, the two discovery passes, the note registry, the report —
//! lives in `strk20-consumer` and is the *same code the native CLI runs*. That
//! is the entire point: the browser is not a second implementation that is
//! supposed to agree, it is the first one, recompiled.
//!
//! # The contract
//!
//! **No network, no storage, no async crosses this boundary.** TypeScript owns
//! `fetch`, IndexedDB, zstd inflation, SSE — and the one JSON-RPC round trip
//! §1.5 ring 6 needs against the *user's own* node; it pushes bytes in and
//! takes JSON out. Consequences, all deliberate:
//!
//! * no `tokio`, no `reqwest`, no `rusqlite`, no `web-sys`, no `getrandom`;
//! * no `zstd` — `zstd-sys` compiles C and has no wasm32 backend, so the module
//!   takes **already-inflated** payloads. Content identity is over uncompressed
//!   bytes everywhere in this system, so nothing verification-bearing is lost;
//!   TypeScript must check the `.zst` sha256 *before* inflating, and cap the
//!   output;
//! * every ABI method is synchronous. Block B's `async` is driven by
//!   [`drive::drive`], a single `poll` over futures that cannot suspend.
//!
//! # Key handling
//!
//! **The viewing key enters this module and does not leave it.** It appears in
//! exactly one entry point, [`Engine::discover`], as a `&mut [u8]`:
//!
//! * it is never returned, never embedded in an error `message` or `details`,
//!   never logged — this crate has no logging sink at all, and the one type
//!   that holds it (`discovery_core::SecretFelt`) renders as `[REDACTED]` and
//!   zeroes itself on drop;
//! * it is never written into the state blob. That container carries feed
//!   artifacts only (see [`blob`]); discovery cursors — which *do* hold
//!   key-derived channel keys — live in the module's in-memory store and are
//!   not exported by any method;
//! * the staging buffer is zeroized in place before `discover` returns. Because
//!   `wasm-bindgen` copies a `&mut [u8]` back out to the caller's
//!   `Uint8Array`, that zeroing reaches the JS-side buffer too. Pass the key as
//!   a `Uint8Array`, never a string: JS strings are immutable and cannot be
//!   cleared.
//!
//! **The honest limit**, which belongs in any README that wraps this module:
//! the guarantee is **non-transmission** — the module never sends the key
//! anywhere and zeroes what it owns. It is *not* memory hygiene in the host.
//! JavaScript cannot reliably zeroize its own buffers, wasm linear memory is
//! readable by the page, and a copy the caller made before calling is the
//! caller's problem.

#![deny(unsafe_code)]

pub mod blob;
pub mod drive;
pub mod proofs;
pub mod staged;

mod err;

pub use engine::*;

/// The `#[wasm_bindgen]` facade — the crate's one `unsafe_code` exemption.
///
/// `#[wasm_bindgen]` expands to `unsafe` items (the generated `__wbg_*_free`
/// externs and ABI shims). `deny` can be lifted here; `forbid` could not be,
/// which is why the crate uses `deny`. There is no hand-written `unsafe` block
/// anywhere in this crate.
#[allow(unsafe_code)]
mod engine {
    use crate::blob::{self, StateHeader, BLOB_VERSION, ENGINE_VERSION};
    use crate::drive::drive;
    use crate::err::{to_js, ErrJson};
    use crate::proofs::StagedProofs;
    use crate::staged::StagedFeed;
    use anyhow::{anyhow, bail, Result};
    use discovery_core::privacy_pool::types::SecretFelt;
    use serde_json::json;
    use starknet_types_core::felt::Felt;
    use std::sync::Arc;
    use strk20_consumer::anchors::{grounding_candidates, ProofSource};
    use strk20_consumer::mem::MemStore;
    use strk20_consumer::store::{ColdStart, ConsumerStore};
    use strk20_consumer::sync::{sync_once, SyncOptions};
    use strk20_feed::manifest::{Genesis, Manifest};
    use wasm_bindgen::prelude::*;
    use zeroize::Zeroize;

    /// One consumer: an in-memory mirror plus the bytes it was folded from.
    ///
    /// Cheap to construct, expensive to fold. Hold one per feed per tab.
    #[wasm_bindgen]
    pub struct Engine {
        store: MemStore,
        feed: StagedFeed,
        proofs: Arc<StagedProofs>,
        genesis: Genesis,
        /// The grade ring 6 last ESTABLISHED, if any.
        ///
        /// `info()` has to be able to answer "what has this mirror earned?"
        /// before a caller runs `discover`, and the floor — `replayed` vs
        /// `server-asserted` — follows from `snapshot_basis` alone. `anchored`
        /// does not: it is only known once ring 6 has run, which happens
        /// inside `discover`. Without this field `info().verified` would have
        /// an unreachable arm, which is precisely the defect this replaces on
        /// the TypeScript side. Cleared by every `apply`, because a new head
        /// moves the candidate blocks and an old verdict does not survive it.
        grade: std::sync::Mutex<Option<String>>,
    }

    fn felt(hex: &str, what: &str) -> Result<Felt> {
        strk20_feed::felt_from_hex(hex)
            .map_err(|e| anyhow!("CHAIN_MISMATCH: {what} {hex:?} is not a felt: {e}"))
    }

    fn meta_u64<S: ConsumerStore>(store: &S, key: &str) -> Option<u64> {
        store.meta_get(key).ok().flatten().and_then(|s| s.parse().ok())
    }

    fn cold_start_of(s: &str) -> Result<ColdStart> {
        match s {
            "auto" | "" => Ok(ColdStart::Auto),
            "snapshot" => Ok(ColdStart::Snapshot),
            "epochs" => Ok(ColdStart::Epochs),
            other => Err(anyhow!(
                "CONFIG_INVALID: cold_start {other:?} is not \"auto\", \"snapshot\" or \"epochs\""
            )),
        }
    }

    #[wasm_bindgen]
    impl Engine {
        /// Pin identity from the fetched `genesis.json` bytes. Every artifact
        /// staged afterwards is checked against this document.
        #[wasm_bindgen(constructor)]
        pub fn new(genesis_json: &str) -> Result<Engine, JsError> {
            to_js(Self::build(genesis_json))
        }

        fn build(genesis_json: &str) -> Result<Engine> {
            let feed = StagedFeed::new();
            let genesis = feed.set_genesis(genesis_json)?;
            Ok(Engine {
                store: MemStore::new(),
                feed,
                proofs: Arc::new(StagedProofs::new()),
                genesis,
                grade: std::sync::Mutex::new(None),
            })
        }

        fn set_grade(&self, grade: Option<String>) {
            *self.grade.lock().expect("grade poisoned") = grade;
        }

        /// The grade this mirror has earned, by the module's own rule. The
        /// floor follows from `snapshot_basis`; `anchored` is remembered from
        /// the last `discover` that ring 6 grounded.
        fn grade_now(&self) -> &'static str {
            if meta_u64(&self.store, "snapshot_basis").is_none() {
                return "replayed";
            }
            match self.grade.lock().expect("grade poisoned").as_deref() {
                Some("anchored") => "anchored",
                _ => "server-asserted",
            }
        }

        /// Engine semver — the `engine` field of a state blob's stamp.
        pub fn version() -> String {
            ENGINE_VERSION.to_owned()
        }

        // ------------------------------------------------------------ staging
        //
        // Staging is push-only bookkeeping: it parses and stores, it does not
        // fold. `apply` folds. The split exists because Block B's `apply_feed`
        // is one incremental pass over everything available, not a sequence of
        // per-artifact applies — see README, "Where §A3 was wrong".

        /// The fetched `manifest.json`. Required before any `apply`.
        pub fn stage_manifest(&self, manifest_json: &str) -> Result<(), JsError> {
            to_js(self.feed.set_manifest(manifest_json))
        }

        /// One epoch's **inflated** payload. Its sha256, its header range and
        /// its prev-linkage are all checked by `apply`, so staging the wrong
        /// bytes fails there rather than here.
        pub fn stage_epoch(&self, e: u64, payload: &[u8]) {
            self.feed.put_epoch(e, payload.to_vec());
        }

        /// The snapshot, **both halves**: `zst` is the compressed file exactly
        /// as served (ring 1 of the §1.5 ladder hashes it here, in Rust), and
        /// `payload` is what TypeScript inflated from it (rings 2-5 parse
        /// that). Passing a mismatched pair fails `apply`.
        pub fn stage_snapshot(&self, e: u64, zst: &[u8], payload: &[u8]) {
            self.feed.put_snapshot(e, zst.to_vec(), payload.to_vec());
        }

        /// The `snapshots/{e:08}.anchor.json` sidecar, when the feed publishes
        /// one (§12 point 1).
        pub fn stage_snapshot_anchor(&self, e: u64, json: &[u8]) {
            self.feed.put_snapshot_anchor(e, json.to_vec());
        }

        /// `anchors.ndjson`. Required for a snapshot cold start: the §11.3
        /// reachability walk is what grounds the snapshot's slot set.
        pub fn stage_anchors(&self, payload: &[u8]) {
            self.feed.put_anchors(payload.to_vec());
        }

        /// `head.ndjson` and the ETag it was served with. Re-staging the same
        /// ETag makes the next `apply` skip the tail rebuild, exactly as the
        /// native client's conditional GET does.
        pub fn stage_head(&self, payload: &[u8], etag: &str) {
            self.feed.put_head(payload.to_vec(), etag.to_owned());
        }

        /// §1.5 ring 6, staged: one `starknet_getStorageProof` answer for
        /// `block`, from the endpoint the USER chose.
        ///
        /// This is the one input that takes the feed server out of the trust
        /// path, and it is what lifts `discover`'s `verified` from
        /// `"server-asserted"` to `"anchored"`. Call [`Engine::proof_candidates`]
        /// first: it names the blocks ring 6 will actually ask about, and a
        /// proof for any other block is refused rather than ignored.
        ///
        /// **One argument, deliberately.** An earlier signature also took the
        /// `block_hash` the caller's `starknet_getBlockWithTxHashes(block)`
        /// returned, and compared the two — but both came from the caller in
        /// the same call, so the comparison proved only that the caller agreed
        /// with itself. The pin is now made against a hash the MODULE holds
        /// (`mirror_block_hash`), inside ring 6, and no second RPC round trip
        /// is needed. See [`crate::proofs`].
        ///
        /// `proof_json` may be the whole JSON-RPC envelope or just its
        /// `result` object.
        pub fn stage_storage_proof(&self, block: u64, proof_json: &str) -> Result<(), JsError> {
            to_js(self.proofs.put(block, proof_json))
        }

        /// Drop every staged proof. A new head moves the candidate blocks, so
        /// a live client re-stages instead of accumulating.
        pub fn clear_storage_proofs(&self) {
            self.proofs.clear();
        }

        /// The blocks ring 6 will ask about, in the order it will ask —
        /// computed by Block B, not by this wrapper, so the two cannot drift.
        ///
        /// Returns `{"pool","basis","head","blocks","staged","reason"}`.
        /// `blocks` is empty with a `reason` when grounding cannot run at all:
        /// an epoch-replayed mirror has no snapshot basis, so its grade is
        /// `"replayed"` and no proof is consulted.
        pub fn proof_candidates(&self) -> Result<String, JsError> {
            to_js(self.proof_candidates_inner())
        }

        fn proof_candidates_inner(&self) -> Result<String> {
            let head = meta_u64(&self.store, "head_number").unwrap_or(0);
            let basis = meta_u64(&self.store, "snapshot_basis");
            let (blocks, reason) = match basis {
                Some(basis) => {
                    let blocks = drive(grounding_candidates(&self.feed, basis, head))?;
                    let reason = if blocks.is_empty() {
                        Some(format!(
                            "no block at or above the snapshot basis {basis} is available to \
                             ground against (mirror head {head})"
                        ))
                    } else {
                        None
                    };
                    (blocks, reason)
                }
                None => (
                    Vec::new(),
                    Some(
                        "this mirror was replayed from the epoch chain, not cold-started from \
                         a snapshot, so ring 6 does not run and the grade is \"replayed\""
                            .to_owned(),
                    ),
                ),
            };
            Ok(json!({
                "pool": self.genesis.pool,
                "basis": basis,
                "head": head,
                "blocks": blocks,
                "staged": self.proofs.staged_blocks(),
                "reason": reason,
            })
            .to_string())
        }

        // ------------------------------------------------------------ folding

        /// Fold everything staged: snapshot ladder (on an empty mirror), epoch
        /// hash chain, chain/pool binding, tail rebuild, reorg supersede.
        ///
        /// Incremental and idempotent — already-applied epochs are skipped and
        /// an unchanged ETag skips the tail — so this is also the live path.
        ///
        /// `cold_start` is `"auto" | "snapshot" | "epochs"`.
        /// Returns `{"epochs_applied","last_epoch","last_epoch_to","head",
        /// "l1_accepted","tail_rewound","history_floor","snapshot_basis",
        /// "snapshot_rejected","state_changed"}`.
        pub fn apply(&self, cold_start: &str) -> Result<String, JsError> {
            to_js(self.apply_inner(cold_start))
        }

        fn apply_inner(&self, cold_start: &str) -> Result<String> {
            let mode = cold_start_of(cold_start)?;
            // A fold moves the mirror; a grade established against the old one
            // does not carry over.
            self.set_grade(None);
            let before = (
                meta_u64(&self.store, "last_epoch_applied"),
                meta_u64(&self.store, "snapshot_basis"),
            );
            let out = drive(strk20_consumer::apply::apply_feed(
                &self.store,
                &self.feed,
                mode,
            ))?;
            let after = (
                meta_u64(&self.store, "last_epoch_applied"),
                meta_u64(&self.store, "snapshot_basis"),
            );
            Ok(json!({
                "epochs_applied": out.epochs_applied,
                "last_epoch": after.0,
                "last_epoch_to": out.last_epoch_to,
                "head": out.head,
                "l1_accepted": out.l1_accepted,
                "tail_rewound": out.tail_rewound,
                "history_floor": out.history_floor,
                "snapshot_basis": out.snapshot_basis,
                "snapshot_rejected": out.snapshot_rejected,
                // The field §4.3's export rule reads: only epoch-derived state
                // is exportable, so a tail-only change is NOT a state change.
                "state_changed": before != after,
            })
            .to_string())
        }

        /// Stage a new head tail and fold it — the SSE hot path, one call.
        /// Returns `{"head","l1_accepted","tail_rewound"}`.
        pub fn apply_head(&self, payload: &[u8], etag: &str) -> Result<String, JsError> {
            to_js((|| {
                self.set_grade(None);
                self.feed.put_head(payload.to_vec(), etag.to_owned());
                let out = drive(strk20_consumer::apply::apply_feed(
                    &self.store,
                    &self.feed,
                    ColdStart::Auto,
                ))?;
                Ok(json!({
                    "head": out.head,
                    "l1_accepted": out.l1_accepted,
                    "tail_rewound": out.tail_rewound,
                })
                .to_string())
            })())
        }

        // ------------------------------------------------------------ reading

        /// `{"chain_id","pool","genesis_block","epoch_size","last_epoch",
        /// "last_epoch_hash","last_epoch_to","history_floor","snapshot_basis",
        /// "head","l1_accepted","slots","tail_generation","verified",
        /// "engine_version"}`.
        ///
        /// `verified` is the trust grade, decided by the module. Read it; do
        /// not re-derive it from `snapshot_basis` in the wrapper.
        pub fn info(&self) -> Result<String, JsError> {
            to_js(self.info_inner())
        }

        fn info_inner(&self) -> Result<String> {
            let head = meta_u64(&self.store, "head_number").unwrap_or(0);
            let slots = self.store.full_slot_set_as_of(head)?.len();
            Ok(json!({
                "chain_id": self.genesis.chain_id,
                "pool": self.genesis.pool,
                "genesis_block": self.genesis.genesis_block,
                "epoch_size": self.genesis.epoch_size,
                "last_epoch": meta_u64(&self.store, "last_epoch_applied"),
                "last_epoch_hash": self.store.meta_get("last_epoch_hash")?,
                "last_epoch_to": meta_u64(&self.store, "last_epoch_to").unwrap_or(0),
                "history_floor": meta_u64(&self.store, "history_floor").unwrap_or(0),
                "snapshot_basis": meta_u64(&self.store, "snapshot_basis"),
                "head": head,
                "l1_accepted": meta_u64(&self.store, "l1_accepted").unwrap_or(0),
                "slots": slots,
                "tail_generation": self.store.tail_generation()?,
                // The trust grade, decided HERE and not re-derived by the
                // wrapper. A wrapper that recomputes it has to encode the rule
                // a second time, and the second copy in this project's
                // TypeScript could not express `anchored` at all.
                "verified": self.grade_now(),
                "engine_version": ENGINE_VERSION,
            })
            .to_string())
        }

        /// Arbitrate staleness against a freshly fetched manifest — all of it,
        /// in Rust. Returns `"ok" | "behind" | "diverged"`.
        ///
        /// Staleness is a **return value, never a throw** (§3.7): a blob that
        /// is unusable rather than merely stale already throws from `load`.
        pub fn check_manifest(&self, manifest_json: &str) -> Result<String, JsError> {
            to_js(self.check_manifest_inner(manifest_json))
        }

        fn check_manifest_inner(&self, manifest_json: &str) -> Result<String> {
            let m: Manifest = serde_json::from_str(manifest_json)
                .map_err(|e| anyhow!("FEED_MALFORMED: manifest.json is not a manifest: {e}"))?;
            if m.chain_id != self.genesis.chain_id
                || felt(&m.pool, "manifest pool")? != felt(&self.genesis.pool, "genesis pool")?
            {
                return Ok("diverged".into());
            }
            let Some(applied) = meta_u64(&self.store, "last_epoch_applied") else {
                let empty = m.epochs.is_empty() && m.snapshot.is_none();
                return Ok(if empty { "ok" } else { "behind" }.into());
            };
            let local_hash = self.store.meta_get("last_epoch_hash")?.unwrap_or_default();
            match m.epoch(applied) {
                // The manifest no longer lists, or no longer agrees about, an
                // epoch we already folded: this feed is not ours.
                None => return Ok("diverged".into()),
                Some(entry) if entry.hash != local_hash => return Ok("diverged".into()),
                Some(_) => {}
            }
            if m.latest_epoch.is_some_and(|latest| latest > applied) {
                return Ok("behind".into());
            }
            let head = meta_u64(&self.store, "head_number").unwrap_or(0);
            Ok(if m.head.number > head { "behind" } else { "ok" }.into())
        }

        // ------------------------------------------------------- persistence

        /// The persisted state blob: the verified feed artifacts, **never the
        /// head tail and never per-key material**. Call after an `apply` that
        /// reported `state_changed`. See [`crate::blob`] for how this differs
        /// from §3.5 and why.
        pub fn export_state(&self) -> Result<Vec<u8>, JsError> {
            to_js(self.export_inner())
        }

        fn export_inner(&self) -> Result<Vec<u8>> {
            let header = StateHeader {
                v: BLOB_VERSION,
                kind: "strk20-state".into(),
                engine: ENGINE_VERSION.into(),
                chain_id: self.genesis.chain_id.clone(),
                pool: self.genesis.pool.clone(),
                genesis_block: self.genesis.genesis_block,
                epoch_size: self.genesis.epoch_size,
                last_epoch: meta_u64(&self.store, "last_epoch_applied"),
                last_epoch_hash: self.store.meta_get("last_epoch_hash")?,
                last_epoch_to: meta_u64(&self.store, "last_epoch_to").unwrap_or(0),
                history_floor: meta_u64(&self.store, "history_floor").unwrap_or(0),
                snapshot_basis: meta_u64(&self.store, "snapshot_basis"),
            };
            blob::encode(&header, &self.feed.snapshot_of_staged())
        }

        /// Restore from a state blob: verify the trailer hash and the identity
        /// stamp against `genesis_json`, then **re-stage** every artifact the
        /// blob carries.
        ///
        /// This restores bytes, not a folded mirror. The caller must
        /// `stage_head` and `apply` afterwards — which is the flow anyway,
        /// because the tail is never exported and a client always has to fetch
        /// a live head. Folding here instead would be actively wrong: with no
        /// tail staged the mirror's head is 0, the §11.3 reachability walk
        /// finds no anchor at or below it, the snapshot is rejected as
        /// ungrounded, and `auto` silently falls back to a full epoch replay —
        /// exactly the cost a saved state blob exists to avoid.
        ///
        /// Never partially loads: the engine is built aside and returned only
        /// if the whole decode succeeds.
        pub fn load(blob_bytes: &[u8], genesis_json: &str) -> Result<Engine, JsError> {
            to_js((|| {
                let engine = Engine::build(genesis_json)?;
                blob::decode_into(blob_bytes, &engine.genesis, &engine.feed)?;
                Ok(engine)
            })())
        }

        // --------------------------------------------------------- discovery

        /// One full discovery pass for one owner: the checkpoint pass at
        /// `last_epoch_to`, the live pass at `head`, and the spent refresh —
        /// the exact two-pass structure of the native `sync_once`, because it
        /// *is* `sync_once`.
        ///
        /// `key` is the 32-byte big-endian viewing key. It is zeroized in place
        /// before this returns, and `wasm-bindgen` copies that zeroing back
        /// into the caller's `Uint8Array`. It is never returned, logged or
        /// persisted.
        ///
        /// Returns the canonical `SyncReport` JSON — field-identical to
        /// `strk20-sync sync --json`: notes, balances, spent-state, senders,
        /// recipients, completion flags, `history_from`, `verified`.
        ///
        /// `verified` is `"replayed"` when the epoch chain was folded from
        /// genesis, `"anchored"` when a staged storage proof grounded the
        /// mirror in the chain (§1.5 ring 6 — see [`Engine::stage_storage_proof`]),
        /// and `"server-asserted"` when a snapshot start was grounded only by
        /// an anchor the FEED published. A staged proof that fails to verify
        /// throws; it never degrades the grade silently.
        pub fn discover(&self, owner_hex: &str, key: &mut [u8]) -> Result<String, JsError> {
            // Zeroize on EVERY path, including the error ones, before the
            // result can be inspected.
            let out = self.discover_inner(owner_hex, key);
            key.zeroize();
            to_js(out)
        }

        fn discover_inner(&self, owner_hex: &str, key: &mut [u8]) -> Result<String> {
            if key.len() != 32 {
                return Err(anyhow!(
                    "KEY_INVALID: the viewing key must be exactly 32 big-endian bytes, got {}",
                    key.len()
                ));
            }
            let owner = felt(owner_hex, "owner")?;
            let mut raw = [0u8; 32];
            raw.copy_from_slice(key);
            let secret = SecretFelt::new(Felt::from_bytes_be(&raw));
            raw.zeroize();

            // Ring 6 runs iff the caller staged a proof. The network call is
            // TypeScript's — see `stage_storage_proof` — but the decision made
            // from the answer is Block B's, unchanged and shared with the
            // native client.
            let staged = self.proofs.staged_blocks();
            let opts = SyncOptions {
                cold_start: ColdStart::Auto,
                anchor_proofs: (!staged.is_empty())
                    .then(|| Arc::clone(&self.proofs) as Arc<dyn ProofSource>),
            };
            self.proofs.begin();
            let report = drive(sync_once(&self.store, &self.feed, owner, &secret, &opts))?;
            // `secret` drops here and zeroes itself; the report has never held
            // key material.

            // A staged proof that nothing consumed must NOT come back as a
            // quietly weaker grade. "server-asserted" would then be
            // indistinguishable from "anchored, and the check passed", which
            // makes the strong grade unfalsifiable — the one failure mode this
            // ring exists to rule out. A refuted proof already fails inside
            // Block B (ANCHOR_NOT_ON_CHAIN); this covers the other way it can
            // go unused.
            //
            // "replayed" is NOT that case. A mirror folded from genesis has the
            // strongest provenance in the system and ring 6 correctly does not
            // run on it (there is no snapshot to ground). A defensive caller
            // that always stages a proof must not be punished for landing on
            // the one mirror that needed none.
            if !staged.is_empty() && !matches!(report.verified.as_str(), "anchored" | "replayed") {
                let asked = self.proofs.asked();
                let asked_for = if asked.is_empty() {
                    "ring 6 asked about no block at all".to_owned()
                } else {
                    format!("ring 6 asked about {asked:?}")
                };
                bail!(
                    "PROOF_UNUSED: a storage proof is staged for block(s) {staged:?}, but \
                     nothing consumed it — the grade came back {:?}, not \"anchored\". \
                     {asked_for}. Either the proof was for a block ring 6 does not ask \
                     about, or it could not be pinned to a block hash this mirror holds. \
                     Call proof_candidates() and stage a proof for a block it names, or \
                     clear_storage_proofs() to accept {:?} deliberately.",
                    report.verified,
                    report.verified
                );
            }
            // `info().verified` answers with what ring 6 actually established,
            // so the wrapper never has to re-derive it.
            self.set_grade(Some(report.verified.clone()));
            Ok(serde_json::to_string(&report)?)
        }

        /// Drop every cursor and registry row for `owner`. The mirror is kept
        /// and stays verified — this is the recovery path, not a resync.
        pub fn forget_owner(&self, owner_hex: &str) -> Result<(), JsError> {
            to_js((|| {
                let owner = felt(owner_hex, "owner")?;
                strk20_consumer::sync::full_resync(&self.store, &owner)
            })())
        }
    }

    /// Route Rust panics into a readable JS exception instead of an opaque
    /// `unreachable`. Call once at startup. Costs ~4 KB of formatting glue.
    #[wasm_bindgen]
    pub fn set_panic_hook() {
        use std::sync::Once;
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            std::panic::set_hook(Box::new(|info| {
                // Panic payloads in this crate are programming-error strings —
                // never key material, which lives only in `SecretFelt`.
                let msg = ErrJson::internal(&info.to_string()).to_string();
                wasm_bindgen::throw_str(&msg);
            }));
        });
    }
}
