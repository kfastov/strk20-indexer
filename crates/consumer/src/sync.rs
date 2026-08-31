//! Sync orchestration (spec §7): apply the verified feed, then run the
//! UNMODIFIED discovery-core engine locally in two passes — a checkpoint pass
//! bound to the L1-final epoch boundary and a live pass bound to the head.
//! On a tail reorg the live cursor is discarded and the client resumes from
//! the checkpoint — never from scratch (spec §7.5).
//!
//! Every line here is host-independent: the store is a [`ConsumerStore`], the
//! feed is a [`FeedTransport`], and ring 6's chain access is a
//! [`ProofSource`]. That is the whole point — the native CLI and the browser
//! run this exact code, so the equality claim rests on one implementation
//! rather than on two that are supposed to agree.

use crate::anchors::{ground_mirror_against_rpc, Grounding, ProofSource};
use crate::apply::apply_feed;
use crate::store::{ApplyOutcome, ColdStart, ConsumerStore, NoteRow};
use crate::transport::FeedTransport;
use anyhow::{bail, Result};
use discovery_core::discovery::{CursorLimits, DiscoveryCursor};
use discovery_core::io_budget::IoBudget;
use discovery_core::privacy_pool::hashes::compute_nullifier;
use discovery_core::privacy_pool::storage_slots;
use discovery_core::privacy_pool::types::SecretFelt;
use discovery_core::sync::incoming_state::sync_incoming_state;
use discovery_core::sync::outgoing_state::sync_outgoing_state;
use serde::Serialize;
use starknet_types_core::felt::Felt;
use std::sync::Arc;

const PASS_BUDGET: usize = 1_000_000;
const MAX_PASSES: usize = 1000;

#[derive(Debug, Serialize)]
pub struct ReportNote {
    pub token: String,
    pub index: u64,
    pub note_id: String,
    pub nullifier: String,
    pub amount: String,
    pub block_number: u64,
    pub sender: String,
    pub spent: bool,
}

#[derive(Debug, Serialize)]
pub struct SyncReport {
    pub address: String,
    pub head: u64,
    pub l1_accepted: u64,
    pub last_epoch_to: u64,
    pub tail_rewound: bool,
    pub incoming_complete: bool,
    pub outgoing_complete: bool,
    pub incoming_senders: Vec<String>,
    pub outgoing_recipients: Vec<String>,
    pub notes: Vec<ReportNote>,
    pub balances: std::collections::BTreeMap<String, String>,
    pub newly_spent: Vec<String>,
    /// Lowest block for which this mirror holds EVENTS. 0 for a fully
    /// epoch-replayed mirror; `snapshot.block + 1` for a snapshot-started one,
    /// whose transaction history below the floor does not exist locally and
    /// must never be answered with zeros (§1.1).
    pub history_from: u64,
    pub snapshot_basis: Option<u64>,
    /// A snapshot was offered and refused; `auto` fell back to epoch replay.
    pub snapshot_rejected: bool,
    /// §1.5.1 — the integrity grade, surfaced rather than implied:
    /// `"replayed"` (epoch chain from genesis), `"anchored"` (snapshot plus a
    /// ring-6 check against the user's own RPC), `"server-asserted"` (snapshot
    /// grounded only by reachability against an anchor the SERVER published).
    pub verified: String,
}

/// Everything the caller may choose about how a sync is performed. Nothing
/// here is derived from a user: cold-start mode is a local preference and the
/// anchor proof source is the user's own endpoint, never the feed's.
#[derive(Clone, Default)]
pub struct SyncOptions {
    pub cold_start: ColdStart,
    /// §1.5 ring 6. When set it RUNS and MUST PASS — there is no
    /// `verify: 'background'` equivalent.
    pub anchor_proofs: Option<Arc<dyn ProofSource>>,
}

impl std::fmt::Debug for SyncOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyncOptions")
            .field("cold_start", &self.cold_start)
            .field(
                "anchor_proofs",
                &self.anchor_proofs.as_ref().map(|p| p.label()),
            )
            .finish()
    }
}

struct CursorKeys {
    live: String,
    ckpt: String,
    ckpt_at: String,
}

fn keys(kind: &str, owner: &Felt) -> CursorKeys {
    let a = strk20_feed::felt_hex(owner);
    CursorKeys {
        live: format!("cur_{kind}_{a}"),
        ckpt: format!("ckpt_{kind}_{a}"),
        ckpt_at: format!("ckpt_at_{kind}_{a}"),
    }
}

/// Re-open a completed cursor for incremental resume. Upstream's cursor is a
/// PAGINATION cursor: completion flags mean "complete as of the block it was
/// computed at", and a complete cursor short-circuits the engine entirely.
/// For a resume at a HIGHER block the completion flags are cleared and the
/// cached totals dropped, while every progress position (last channel /
/// subchannel / note index) is kept — the engine then re-probes only the
/// boundary slots and discovers anything new (the "watch-set grows" property
/// from the research, docs/research/verify-discovery-trace.md §3).
pub fn reopen_cursor(cursor: &mut DiscoveryCursor) {
    cursor.channel_discovery_complete = false;
    cursor.total_n_channels = None;
    for channel in cursor.channels.values_mut() {
        channel.subchannel_discovery_complete = false;
        for sub in channel.subchannels.values_mut() {
            sub.note_discovery_complete = false;
            sub.total_n_notes = None;
        }
    }
}

fn load_cursor<S: ConsumerStore>(store: &S, key: &str) -> Result<Option<DiscoveryCursor>> {
    match store.meta_get(key)?.filter(|s| !s.is_empty()) {
        Some(json) => Ok(Some(serde_json::from_str(&json)?)),
        None => Ok(None),
    }
}

fn save_cursor<S: ConsumerStore>(store: &S, key: &str, cursor: &DiscoveryCursor) -> Result<()> {
    store.meta_set(key, &serde_json::to_string(cursor)?)
}

/// Run one engine flow (incoming or outgoing) to completion at `bound`.
pub async fn run_incoming<S: ConsumerStore>(
    store: &S,
    bound: u64,
    owner: Felt,
    key: &SecretFelt,
    mut cursor: DiscoveryCursor,
) -> Result<(DiscoveryCursor, Vec<discovery_core::discovery::notes::DecryptedNote>)> {
    reopen_cursor(&mut cursor);
    let view = store.view(bound)?;
    let mut notes = Vec::new();
    for _ in 0..MAX_PASSES {
        let budget = IoBudget::new(PASS_BUDGET);
        let out = sync_incoming_state(
            &view,
            owner,
            key,
            cursor,
            CursorLimits::default(),
            &budget,
        )
        .await
        .map_err(|e| anyhow::anyhow!("incoming discovery failed: {e}"))?;
        notes.extend(out.notes);
        cursor = out.cursor;
        if cursor.is_complete() {
            return Ok((cursor, notes));
        }
    }
    bail!("incoming discovery did not complete in {MAX_PASSES} passes")
}

pub async fn run_outgoing<S: ConsumerStore>(
    store: &S,
    bound: u64,
    owner: Felt,
    key: &SecretFelt,
    mut cursor: DiscoveryCursor,
) -> Result<DiscoveryCursor> {
    reopen_cursor(&mut cursor);
    let view = store.view(bound)?;
    for _ in 0..MAX_PASSES {
        let budget = IoBudget::new(PASS_BUDGET);
        let out = sync_outgoing_state(
            &view,
            owner,
            key,
            cursor,
            CursorLimits::default(),
            &budget,
            None,
        )
        .await
        .map_err(|e| anyhow::anyhow!("outgoing discovery failed: {e}"))?;
        cursor = out.cursor;
        if cursor.is_complete() {
            return Ok(cursor);
        }
    }
    bail!("outgoing discovery did not complete in {MAX_PASSES} passes")
}

/// Turn the engine's decrypted notes into registry rows, deriving each
/// nullifier from the channel key the cursor already holds.
pub fn register_notes<S: ConsumerStore>(
    store: &S,
    owner: &Felt,
    key: &SecretFelt,
    cursor: &DiscoveryCursor,
    notes: &[discovery_core::discovery::notes::DecryptedNote],
) -> Result<()> {
    for n in notes {
        let Some(channel) = cursor.channels.get(&n.sender_addr) else {
            tracing::warn!(sender = %strk20_feed::felt_hex(&n.sender_addr), "note without channel cursor");
            continue;
        };
        let nullifier = compute_nullifier(&channel.channel_key, n.token, n.index, key);
        store.upsert_note(&NoteRow {
            note_id: n.note_id,
            owner: *owner,
            sender: n.sender_addr,
            token: n.token,
            index: n.index,
            nullifier,
            amount: n.amount,
            block: n.block_number,
            spent: false,
        })?;
    }
    Ok(())
}

/// Re-evaluate spent-state from the mirror (nullifier slot != 0 as of
/// `block`). Returns nullifiers that flipped to spent.
///
/// Semantics pinned by the live run (findings §7): a spent note's storage slot
/// is NOT cleared, so spentness lives only in the nullifier slot. Anything
/// inferring "unspent" from "the note slot is populated" would be wrong.
pub fn refresh_spent<S: ConsumerStore>(store: &S, owner: &Felt, block: u64) -> Result<Vec<Felt>> {
    let notes = store.notes(owner)?;
    let mut flipped = Vec::new();
    for n in notes {
        let slot = storage_slots::nullifiers(n.nullifier);
        let (value, _) = store.read_slot_as_of(&slot, block)?;
        let is_spent = value != Felt::ZERO;
        if is_spent != n.spent {
            store.set_note_spent(&n.note_id, is_spent)?;
            if is_spent {
                flipped.push(n.nullifier);
            }
        }
    }
    Ok(flipped)
}

/// Drop registry rows whose note slot no longer exists in the mirror — the
/// precise reorg cleanup (covers both direct and masked tail replacements; a
/// canonical note re-added by the new tail/epoch is rediscovered by the next
/// engine pass).
pub fn prune_missing_notes<S: ConsumerStore>(
    store: &S,
    owner: &Felt,
    as_of: u64,
) -> Result<usize> {
    let notes = store.notes(owner)?;
    let mut pruned = 0;
    for n in notes {
        let slot = storage_slots::notes(n.note_id);
        let (value, _) = store.read_slot_as_of(&slot, as_of)?;
        if value == Felt::ZERO {
            store.delete_note(&n.note_id)?;
            pruned += 1;
        }
    }
    Ok(pruned)
}

/// Refuse a feed built for a different chain BEFORE a single epoch is applied.
/// The chain id is stamped in both genesis.json and the manifest; either
/// disagreeing with what the client was told it is on is fatal.
pub async fn check_chain_id(transport: &dyn FeedTransport, expected: &str) -> Result<()> {
    let genesis = transport.fetch_genesis().await?;
    let manifest = transport.fetch_manifest().await?;
    for (source, found) in [("genesis", &genesis.chain_id), ("manifest", &manifest.chain_id)] {
        if found != expected {
            bail!("feed {source} chain id {found} is not the expected chain {expected}");
        }
    }
    Ok(())
}

/// Drop every cursor and registry row for `owner` (recovery path; the
/// mirror itself is kept and stays verified).
pub fn full_resync<S: ConsumerStore>(store: &S, owner: &Felt) -> Result<()> {
    let a = strk20_feed::felt_hex(owner);
    for kind in ["in", "out"] {
        store.meta_set(&format!("cur_{kind}_{a}"), "")?;
        store.meta_set(&format!("ckpt_{kind}_{a}"), "")?;
        store.meta_set(&format!("ckpt_at_{kind}_{a}"), "")?;
    }
    store.delete_owner_notes(owner)?;
    Ok(())
}

/// Canonical hex list from a cursor's channel keys.
///
/// Upstream's cursor stores channels in a `HashMap`, so iterating it yields a
/// different order per map instance. Emitting that order straight into the
/// report made two runs over identical bytes produce different JSON — which
/// the store-equality conformance leg caught, and which would have made the
/// single golden report oracle (§0.4: one schema for the CLI, the wasm module,
/// `serve` and npm) unpinnable. The report is a canonical document: sort.
fn sorted_hex<'a>(keys: impl Iterator<Item = &'a Felt>) -> Vec<String> {
    let mut felts: Vec<&Felt> = keys.collect();
    felts.sort_by_key(|f| f.to_bytes_be());
    felts.into_iter().map(strk20_feed::felt_hex).collect()
}

/// One full keyless sync for `owner`. The viewing key never leaves this
/// process: it is used only to drive local decryption over the mirror.
pub async fn sync_once<S: ConsumerStore>(
    store: &S,
    transport: &dyn FeedTransport,
    owner: Felt,
    key: &SecretFelt,
    opts: &SyncOptions,
) -> Result<SyncReport> {
    let outcome: ApplyOutcome = apply_feed(store, transport, opts.cold_start).await?;

    // §1.5 ring 6 — the ONLY ring that grounds this mirror in the chain itself.
    // Address-blind by construction: the request names a public pool and a
    // public block, so it is identical for every user and the feed server stays
    // outside the proof path.
    //
    // "Configured means mandatory" applies to the one outcome that is evidence
    // about the data: a MISMATCH fails the sync. A capability gap is not that
    // (§11.4/§11.5) — an endpoint that does not implement getStorageProof, or
    // whose window has moved past every block we can ask about, has said
    // nothing, and failing the sync for it is LIVE-6.
    let grounded = match (&opts.anchor_proofs, outcome.snapshot_basis) {
        (Some(proofs), Some(basis)) => {
            let outcome6 = ground_mirror_against_rpc(
                store,
                transport,
                proofs.as_ref(),
                basis,
                outcome.head,
            )
            .await;
            let outcome6 = match outcome6 {
                Ok(o) => o,
                Err(e) => {
                    // The user's own RPC has PROVEN this mirror is not the
                    // chain's. Leaving the rows on disk is how one rejection
                    // becomes a permanently poisoned db: the next sync sees a
                    // non-empty mirror, never re-enters the snapshot branch,
                    // and happily builds on the slot set that was just refuted.
                    store.reset_mirror()?;
                    return Err(e);
                }
            };
            match outcome6 {
                Grounding::Anchored(block) => {
                    tracing::info!(block, "mirror grounded against your own RPC (ring 6)");
                    true
                }
                Grounding::Unavailable(why) => {
                    tracing::warn!(
                        rpc = %proofs.label(),
                        reason = %why,
                        "ring 6 could not run: this endpoint cannot serve a storage proof \
                         for any block we can ask about. That is a statement about the \
                         ENDPOINT, not about the mirror, so the sync stands and the grade \
                         stays server-asserted."
                    );
                    false
                }
            }
        }
        _ => false,
    };
    let verified = match (outcome.snapshot_basis, grounded) {
        (None, _) => "replayed",
        (Some(_), true) => "anchored",
        (Some(_), false) => "server-asserted",
    };
    if verified == "server-asserted" {
        tracing::warn!(
            "integrity grade is server-asserted: the snapshot's slot set is attested only \
             by an anchor the feed itself published. Configure --verify-anchor <rpc> for \
             \"anchored\", or --cold-start epochs for \"replayed\"."
        );
    }
    let in_keys = keys("in", &owner);
    let out_keys = keys("out", &owner);

    // Per-owner reorg rewind via the persisted tail generation (crash-safe
    // and shared-store-safe — review findings): the generation is bumped in
    // the same store write as any tail replacement; every owner whose cursors
    // were computed at an older generation rewinds, regardless of who
    // consumed the ETag edge or whether the process died in between.
    let generation = store.tail_generation()?;
    let gen_key = format!("gen_{}", strk20_feed::felt_hex(&owner));
    let owner_gen: u64 = store
        .meta_get(&gen_key)?
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let rewound_for_owner = owner_gen != generation;
    if rewound_for_owner {
        // Discard live cursors and any registry rows whose note slot is gone
        // from the mirror; resume from the L1-final checkpoint (spec §7.5).
        store.meta_set(&in_keys.live, "")?;
        store.meta_set(&out_keys.live, "")?;
        let pruned = prune_missing_notes(store, &owner, outcome.head)?;
        tracing::info!(
            floor = outcome.last_epoch_to,
            pruned,
            "tail replaced (generation {owner_gen} -> {generation}): rewound to checkpoint"
        );
    }

    // --------------------------- checkpoint pass (bound = last epoch end)
    if outcome.last_epoch_to > 0 {
        let stored_at: Option<u64> = store
            .meta_get(&in_keys.ckpt_at)?
            .and_then(|s| s.parse().ok());
        if stored_at != Some(outcome.last_epoch_to) {
            let ck_in = load_cursor(store, &in_keys.ckpt)?.unwrap_or_default();
            let (cursor, notes) =
                run_incoming(store, outcome.last_epoch_to, owner, key, ck_in).await?;
            register_notes(store, &owner, key, &cursor, &notes)?;
            save_cursor(store, &in_keys.ckpt, &cursor)?;
            store.meta_set(&in_keys.ckpt_at, &outcome.last_epoch_to.to_string())?;

            let ck_out = load_cursor(store, &out_keys.ckpt)?.unwrap_or_default();
            let cursor = run_outgoing(store, outcome.last_epoch_to, owner, key, ck_out).await?;
            save_cursor(store, &out_keys.ckpt, &cursor)?;
            store.meta_set(&out_keys.ckpt_at, &outcome.last_epoch_to.to_string())?;
        }
    }

    // --------------------------------------- live pass (bound = head)
    let live_start_in = match store.meta_get(&in_keys.live)?.filter(|s| !s.is_empty()) {
        Some(json) => serde_json::from_str(&json)?,
        None => load_cursor(store, &in_keys.ckpt)?.unwrap_or_default(),
    };
    let (in_cursor, in_notes) = run_incoming(store, outcome.head, owner, key, live_start_in).await?;
    register_notes(store, &owner, key, &in_cursor, &in_notes)?;
    save_cursor(store, &in_keys.live, &in_cursor)?;

    let live_start_out = match store.meta_get(&out_keys.live)?.filter(|s| !s.is_empty()) {
        Some(json) => serde_json::from_str(&json)?,
        None => load_cursor(store, &out_keys.ckpt)?.unwrap_or_default(),
    };
    let out_cursor = run_outgoing(store, outcome.head, owner, key, live_start_out).await?;
    save_cursor(store, &out_keys.live, &out_cursor)?;

    // ------------------------------------------------ spent-state refresh
    let newly_spent = refresh_spent(store, &owner, outcome.head)?;
    // Cursors for this owner are now consistent with the current tail.
    store.meta_set(&gen_key, &generation.to_string())?;

    // ------------------------------------------------------------- report
    let notes = store.notes(&owner)?;
    let mut balances: std::collections::BTreeMap<String, u128> = Default::default();
    for n in notes.iter().filter(|n| !n.spent) {
        *balances
            .entry(strk20_feed::felt_hex(&n.token))
            .or_default() += n.amount;
    }
    Ok(SyncReport {
        address: strk20_feed::felt_hex(&owner),
        head: outcome.head,
        l1_accepted: outcome.l1_accepted,
        last_epoch_to: outcome.last_epoch_to,
        tail_rewound: rewound_for_owner && generation > 0,
        incoming_complete: in_cursor.is_complete(),
        outgoing_complete: out_cursor.is_complete(),
        incoming_senders: sorted_hex(in_cursor.channels.keys()),
        outgoing_recipients: sorted_hex(out_cursor.channels.keys()),
        notes: notes
            .iter()
            .map(|n| ReportNote {
                token: strk20_feed::felt_hex(&n.token),
                index: n.index,
                note_id: strk20_feed::felt_hex(&n.note_id),
                nullifier: strk20_feed::felt_hex(&n.nullifier),
                amount: n.amount.to_string(),
                block_number: n.block,
                sender: strk20_feed::felt_hex(&n.sender),
                spent: n.spent,
            })
            .collect(),
        balances: balances
            .into_iter()
            .map(|(k, v)| (k, v.to_string()))
            .collect(),
        newly_spent: newly_spent.iter().map(strk20_feed::felt_hex).collect(),
        history_from: outcome.history_floor,
        snapshot_basis: outcome.snapshot_basis,
        snapshot_rejected: outcome.snapshot_rejected,
        verified: verified.to_owned(),
    })
}
