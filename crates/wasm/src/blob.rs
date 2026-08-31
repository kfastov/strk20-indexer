//! The persisted state blob (`export_state` / `Engine.load`).
//!
//! # This is NOT §3.5's blob, and the difference is load-bearing
//!
//! §3.5 specifies a blob of *folded state*: one `s` line per storage slot, one
//! `b` line per block, one `ev` line per event, as of `last_epoch_to`. That
//! cannot be produced from outside `strk20-consumer` today — `ConsumerStore`
//! exposes no way to enumerate events at all, and `full_slot_set_as_of` drops
//! each slot's write block, which §3.5's `w` field requires. A wrapper crate
//! physically cannot write that document.
//!
//! So this blob carries the **verified feed artifacts** instead — genesis, the
//! manifest, the applied epochs, the snapshot and its sidecar, the anchors log —
//! and `load` replays them through the same `apply_feed` a cold start uses.
//!
//! What that costs: the blob is larger than a folded one for a long epoch
//! replay, and `load` pays the fold again (for a snapshot-started client, the
//! usual case, that is one snapshot plus zero or one epochs).
//!
//! What it buys, and why it is not merely a fallback: `load` **re-verifies**.
//! The epoch hash chain, the epoch/pool/chain binding, the §1.5 snapshot ladder
//! and the §11.3 reachability walk all run again over the restored bytes. A
//! folded blob authenticated by its own trailer hash proves only that the blob
//! is the one we wrote; this proves the state is the one the feed's hash chain
//! says it is, from a store the browser does not control (IndexedDB is
//! same-origin script-writable).
//!
//! Two §3.5 properties are preserved exactly:
//!
//! * **The tail is never exported.** `head.ndjson` is not a member of this
//!   container, so a reorg cannot stale a saved blob — the tail lives and dies
//!   in memory. This is structural, not a convention: `export` has no access to
//!   a staged head under any name.
//! * **Per-key material is never exported.** Discovery cursors hold channel
//!   keys and live only in the module's `MemStore` meta, which this container
//!   does not read.
//!
//! # Container format
//!
//! ```text
//! "S20STATE"                     8 bytes ASCII magic
//! u32be header_len, header JSON  the compatibility stamp
//! repeat:
//!   u32be name_len, name utf8
//!   u32be payload_len, payload
//! u32be 0xFFFF_FFFF              end marker
//! sha256                         32 bytes over every preceding byte
//! ```

use crate::staged::{StagedExport, StagedFeed};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use strk20_feed::manifest::Genesis;

const MAGIC: &[u8; 8] = b"S20STATE";
const END: u32 = u32::MAX;
pub const BLOB_VERSION: u64 = 1;
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The compatibility stamp of §3.5, kept verbatim in intent: format version,
/// chain identity, engine major, and the hash of the last applied epoch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateHeader {
    pub v: u64,
    pub kind: String,
    pub engine: String,
    pub chain_id: String,
    pub pool: String,
    pub genesis_block: u64,
    pub epoch_size: u64,
    pub last_epoch: Option<u64>,
    pub last_epoch_hash: Option<String>,
    pub last_epoch_to: u64,
    pub history_floor: u64,
    pub snapshot_basis: Option<u64>,
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn put_frame(out: &mut Vec<u8>, name: &str, payload: &[u8]) {
    put_u32(out, name.len() as u32);
    out.extend_from_slice(name.as_bytes());
    put_u32(out, payload.len() as u32);
    out.extend_from_slice(payload);
}

pub fn encode(header: &StateHeader, staged: &StagedExport) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    let head_json = serde_json::to_vec(header)?;
    put_u32(&mut out, head_json.len() as u32);
    out.extend_from_slice(&head_json);

    if let Some(g) = &staged.genesis {
        put_frame(&mut out, "genesis.json", &serde_json::to_vec(g)?);
    }
    if let Some(m) = &staged.manifest {
        put_frame(&mut out, "manifest.json", &serde_json::to_vec(m)?);
    }
    for (e, payload) in &staged.epochs {
        put_frame(&mut out, &format!("epochs/{e}"), payload);
    }
    for (e, (zst, raw)) in &staged.snapshots {
        put_frame(&mut out, &format!("snapshots/{e}.zst"), zst);
        put_frame(&mut out, &format!("snapshots/{e}.raw"), raw);
    }
    for (e, json) in &staged.snapshot_anchors {
        put_frame(&mut out, &format!("snapshots/{e}.anchor.json"), json);
    }
    if let Some(a) = &staged.anchors {
        put_frame(&mut out, "anchors.ndjson", a);
    }
    put_u32(&mut out, END);
    let digest = strk20_feed::payload_sha256(&out);
    out.extend_from_slice(&digest);
    Ok(out)
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .at
            .checked_add(n)
            .filter(|e| *e <= self.b.len())
            .context("STATE_CORRUPT: state blob is truncated")?;
        let out = &self.b[self.at..end];
        self.at = end;
        Ok(out)
    }

    fn u32(&mut self) -> Result<u32> {
        let raw: [u8; 4] = self.take(4)?.try_into().unwrap();
        Ok(u32::from_be_bytes(raw))
    }
}

/// Decode a blob and stage every artifact it carries. Structural and stamp
/// checks only — the *content* checks happen when the caller replays
/// `apply_feed` over the staged bytes, which is the point of this design.
///
/// Never partially applies: the caller builds a throwaway [`StagedFeed`] and
/// keeps it only if the whole function and the subsequent replay succeed.
pub fn decode_into(blob: &[u8], genesis: &Genesis, staged: &StagedFeed) -> Result<StateHeader> {
    if blob.len() < MAGIC.len() + 4 + 32 || &blob[..8] != MAGIC {
        bail!("STATE_CORRUPT: not a strk20 state blob");
    }
    let (body, trailer) = blob.split_at(blob.len() - 32);
    if strk20_feed::payload_sha256(body) != trailer {
        bail!("STATE_CORRUPT: state blob trailer hash does not cover its contents");
    }

    let mut r = Reader { b: body, at: 8 };
    let hlen = r.u32()? as usize;
    let header: StateHeader = serde_json::from_slice(r.take(hlen)?)
        .context("STATE_CORRUPT: state blob header is not a header document")?;

    if header.v != BLOB_VERSION || header.kind != "strk20-state" {
        bail!(
            "STATE_VERSION: state blob is format v{} kind {:?}; this engine reads v{} \
             \"strk20-state\"",
            header.v,
            header.kind,
            BLOB_VERSION
        );
    }
    // Engine MAJOR only: a patch bump must not orphan a user's saved state.
    let major = |s: &str| s.split('.').next().unwrap_or_default().to_owned();
    if major(&header.engine) != major(ENGINE_VERSION) {
        bail!(
            "STATE_VERSION: state blob was written by engine {} and this is {ENGINE_VERSION}",
            header.engine
        );
    }
    for (field, expected, got) in [
        ("chain_id", &genesis.chain_id, &header.chain_id),
        ("pool", &genesis.pool, &header.pool),
    ] {
        if expected != got {
            bail!("STATE_FOREIGN: state blob {field} is {got}, this feed declares {expected}");
        }
    }
    if header.genesis_block != genesis.genesis_block || header.epoch_size != genesis.epoch_size {
        bail!(
            "STATE_FOREIGN: state blob is for genesis_block {} epoch_size {}, this feed \
             declares {} {}",
            header.genesis_block,
            header.epoch_size,
            genesis.genesis_block,
            genesis.epoch_size
        );
    }

    // Snapshots need both halves staged together, so collect first.
    let mut snap_zst: std::collections::BTreeMap<u64, Vec<u8>> = Default::default();
    let mut snap_raw: std::collections::BTreeMap<u64, Vec<u8>> = Default::default();
    loop {
        let nlen = r.u32()?;
        if nlen == END {
            break;
        }
        let name = std::str::from_utf8(r.take(nlen as usize)?)
            .context("STATE_CORRUPT: artifact name is not utf-8")?
            .to_owned();
        let plen = r.u32()? as usize;
        let payload = r.take(plen)?.to_vec();

        // The tail is never exported, so it is never imported either — a blob
        // that carries one was not written by this engine.
        if name == "head.ndjson" {
            bail!("STATE_CORRUPT: state blob carries a head tail, which is never exported");
        }
        match name.as_str() {
            "genesis.json" => {
                staged.set_genesis(std::str::from_utf8(&payload)?)?;
            }
            "manifest.json" => staged.set_manifest(std::str::from_utf8(&payload)?)?,
            "anchors.ndjson" => staged.put_anchors(payload),
            _ => {
                let idx = |s: &str| -> Result<u64> {
                    s.parse()
                        .with_context(|| format!("STATE_CORRUPT: bad artifact name {name:?}"))
                };
                if let Some(e) = name.strip_prefix("epochs/") {
                    staged.put_epoch(idx(e)?, payload);
                } else if let Some(e) = name
                    .strip_prefix("snapshots/")
                    .and_then(|s| s.strip_suffix(".zst"))
                {
                    snap_zst.insert(idx(e)?, payload);
                } else if let Some(e) = name
                    .strip_prefix("snapshots/")
                    .and_then(|s| s.strip_suffix(".raw"))
                {
                    snap_raw.insert(idx(e)?, payload);
                } else if let Some(e) = name
                    .strip_prefix("snapshots/")
                    .and_then(|s| s.strip_suffix(".anchor.json"))
                {
                    staged.put_snapshot_anchor(idx(e)?, payload);
                } else {
                    bail!("STATE_CORRUPT: unknown artifact {name:?} in state blob");
                }
            }
        }
    }
    for (e, zst) in snap_zst {
        let raw = snap_raw
            .remove(&e)
            .with_context(|| format!("STATE_CORRUPT: snapshot {e} has no inflated half"))?;
        staged.put_snapshot(e, zst, raw);
    }
    Ok(header)
}
