//! Sync orchestration (spec §7): apply the verified feed, then run the
//! UNMODIFIED discovery-core engine locally in two passes — a checkpoint pass
//! bound to the L1-final epoch boundary and a live pass bound to the head.
//! On a tail reorg the live cursor is discarded and the client resumes from
//! the checkpoint — never from scratch (spec §7.5).

use crate::store::{ApplyOutcome, FeedStore, NoteRow};
use crate::transport::FeedTransport;
use anyhow::{bail, Result};
use discovery_core::discovery::{CursorLimits, DiscoveryCursor};
use discovery_core::io_budget::IoBudget;
use discovery_core::privacy_pool::hashes::compute_nullifier;
use discovery_core::privacy_pool::types::SecretFelt;
use discovery_core::sync::incoming_state::sync_incoming_state;
use discovery_core::sync::outgoing_state::sync_outgoing_state;
use serde::Serialize;
use starknet_types_core::felt::Felt;

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
fn reopen_cursor(cursor: &mut DiscoveryCursor) {
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

fn load_cursor(store: &FeedStore, key: &str) -> Result<Option<DiscoveryCursor>> {
    match store.meta_get(key)? {
        Some(json) => Ok(Some(serde_json::from_str(&json)?)),
        None => Ok(None),
    }
}

fn save_cursor(store: &FeedStore, key: &str, cursor: &DiscoveryCursor) -> Result<()> {
    store.meta_set(key, &serde_json::to_string(cursor)?)
}

/// Run one engine flow (incoming or outgoing) to completion at `bound`.
async fn run_incoming(
    store: &FeedStore,
    bound: u64,
    owner: Felt,
    key: &SecretFelt,
    mut cursor: DiscoveryCursor,
) -> Result<(DiscoveryCursor, Vec<discovery_core::discovery::notes::DecryptedNote>)> {
    reopen_cursor(&mut cursor);
    let view = store.view(bound);
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

async fn run_outgoing(
    store: &FeedStore,
    bound: u64,
    owner: Felt,
    key: &SecretFelt,
    mut cursor: DiscoveryCursor,
) -> Result<DiscoveryCursor> {
    reopen_cursor(&mut cursor);
    let view = store.view(bound);
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

fn register_notes(
    store: &FeedStore,
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

/// One full keyless sync for `owner`. The viewing key never leaves this
/// process: it is used only to drive local decryption over the mirror.
pub async fn sync_once(
    store: &FeedStore,
    transport: &dyn FeedTransport,
    owner: Felt,
    key: &SecretFelt,
) -> Result<SyncReport> {
    let outcome: ApplyOutcome = store.apply_feed(transport).await?;
    let in_keys = keys("in", &owner);
    let out_keys = keys("out", &owner);

    if outcome.tail_rewound {
        // Reorg: discard live cursors and any tail-derived registry rows;
        // resume from the L1-final checkpoint (spec §7.5).
        store.meta_set(&in_keys.live, "")?;
        store.meta_set(&out_keys.live, "")?;
        store.prune_notes_above(outcome.last_epoch_to)?;
        tracing::info!(
            floor = outcome.last_epoch_to,
            "tail reorg: rewound to L1-final checkpoint"
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
    let newly_spent = store.refresh_spent(&owner, outcome.head)?;

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
        tail_rewound: outcome.tail_rewound,
        incoming_complete: in_cursor.is_complete(),
        outgoing_complete: out_cursor.is_complete(),
        incoming_senders: in_cursor
            .channels
            .keys()
            .map(strk20_feed::felt_hex)
            .collect(),
        outgoing_recipients: out_cursor
            .channels
            .keys()
            .map(strk20_feed::felt_hex)
            .collect(),
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
    })
}
