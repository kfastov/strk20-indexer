//! Deterministic synthetic chain for the acceptance test (spec §10.3):
//! blocks 1..=head with the 48 fixture slots partitioned across blocks
//! {10, 20, 30}, synthesized pool events on every active block, an on-chain
//! class deployed at the first active block, and mutation hooks for the
//! reorg / degraded / spent legs.

use crate::fixture::DevnetFixture;
use starknet_crypto::pedersen_hash;
use starknet_types_core::felt::Felt;
use std::collections::BTreeMap;

/// Live mainnet V2 class hash — used as the fixture pool's class so the
/// binary's default decoder map recognizes it.
pub const KNOWN_CLASS: &str =
    "0x67dddd89d80fedadc06b6f160798f94800a4a70164e5a24301cd0d6076b554d";
pub const ENC_NOTE_CREATED_SELECTOR: &str =
    "0x23c20207be8b1ef4430c25eef8ce779c9745ebe04139555ae81bd4f8fdd6ec5";
pub const NOTE_USED_SELECTOR: &str =
    "0x247fc60d782e0094e7f98c47f277d92a3345d07a436f1f56b27a9b62be2322e";
pub const VIEWING_KEY_SET_SELECTOR: &str =
    "0x1321a492485b4f19851fb787ab3800a0030b595332cba93cd5fe40dfb5a4daf";

#[derive(Debug, Clone)]
pub struct FxEvent {
    pub keys: Vec<Felt>,
    pub data: Vec<Felt>,
}

#[derive(Debug, Clone, Default)]
pub struct ActiveBlock {
    pub diffs: Vec<(Felt, Felt)>,
    pub events: Vec<FxEvent>,
    pub deployed_class: Option<Felt>,
    pub replaced_class: Option<Felt>,
}

#[derive(Debug, Clone)]
pub struct FixtureChain {
    pub pool: Felt,
    pub head: u64,
    pub l1_accepted: u64,
    pub active: BTreeMap<u64, ActiveBlock>,
    /// blocks >= fork_from get their hashes salted with fork_salt
    pub fork_from: u64,
    pub fork_salt: u64,
}

impl FixtureChain {
    /// Initial topology (resolution R6): 48 slots sorted by key, split into
    /// three groups of 16 at blocks {10, 20, 30}; head 46, l1_accepted 40.
    pub fn build(fixture: &DevnetFixture) -> Self {
        let mut slots: Vec<(Felt, Felt)> = fixture.slots.iter().map(|(k, v)| (*k, *v)).collect();
        slots.sort_by_key(|a| a.0.to_bytes_be());
        let mut active = BTreeMap::new();
        let partition_blocks = [10u64, 20, 30];
        let chunk = slots.len().div_ceil(partition_blocks.len());
        for (i, group) in slots.chunks(chunk).enumerate() {
            let number = partition_blocks[i];
            let mut blk = ActiveBlock {
                diffs: group.to_vec(),
                ..Default::default()
            };
            // one synthetic pool event per active block so the events-first
            // scan finds it (the production path, unmodified)
            let selector = if i == 0 {
                VIEWING_KEY_SET_SELECTOR
            } else {
                ENC_NOTE_CREATED_SELECTOR
            };
            blk.events.push(FxEvent {
                keys: vec![
                    Felt::from_hex(selector).unwrap(),
                    group[0].0, // an arbitrary key felt
                ],
                data: vec![Felt::from(i as u64)],
            });
            if number == partition_blocks[0] {
                blk.deployed_class = Some(Felt::from_hex(KNOWN_CLASS).unwrap());
            }
            if i == 1 {
                // Three events on one block (> the fixture RPC's forced page
                // size of 2): regression net for the single-page per-block
                // event truncation found in review.
                for extra in 0..2u64 {
                    blk.events.push(FxEvent {
                        keys: vec![
                            Felt::from_hex(ENC_NOTE_CREATED_SELECTOR).unwrap(),
                            Felt::from(0x9000 + extra),
                        ],
                        data: vec![Felt::from(extra)],
                    });
                }
            }
            active.insert(number, blk);
        }
        Self {
            pool: fixture.constants.contract_address,
            head: 46,
            l1_accepted: 40,
            active,
            fork_from: u64::MAX,
            fork_salt: 0,
        }
    }

    fn number_salt(&self, number: u64) -> u64 {
        if number >= self.fork_from {
            self.fork_salt
        } else {
            0
        }
    }

    /// Deterministic header hashes with parent linkage; forking changes every
    /// hash at and above `fork_from`.
    pub fn block_hash(&self, number: u64) -> Felt {
        let mut h = Felt::from(0x100u64); // "hash" of block 0
        for n in 1..=number {
            h = pedersen_hash(&Felt::from(n + self.number_salt(n) * 1_000_003), &h);
        }
        h
    }

    pub fn parent_hash(&self, number: u64) -> Felt {
        if number == 0 {
            Felt::ZERO
        } else {
            self.block_hash(number - 1)
        }
    }

    pub fn timestamp(&self, number: u64) -> u64 {
        1000 + number
    }

    /// tx hash of event `idx` in block `number`.
    pub fn tx_hash(&self, number: u64, idx: usize) -> Felt {
        pedersen_hash(&self.block_hash(number), &Felt::from(idx as u64 + 1))
    }

    /// Cumulative pool state as of `block` (inclusive) — feeds getStorageProof.
    pub fn state_at(&self, block: u64) -> Vec<(Felt, Felt)> {
        let mut map: BTreeMap<[u8; 32], (Felt, Felt)> = BTreeMap::new();
        for (n, b) in self.active.range(..=block) {
            let _ = n;
            for (slot, value) in &b.diffs {
                map.insert(slot.to_bytes_be(), (*slot, *value));
            }
        }
        map.into_values().collect()
    }

    /// Class hash as of `block`.
    pub fn class_at(&self, block: u64) -> Option<Felt> {
        let mut class = None;
        for (_, b) in self.active.range(..=block) {
            if let Some(c) = &b.deployed_class {
                class = Some(*c);
            }
            if let Some(c) = &b.replaced_class {
                class = Some(*c);
            }
        }
        class
    }

    /// The block whose diff wrote `slot` (committed partition ground truth).
    pub fn write_block_of(&self, slot: &Felt) -> Option<u64> {
        for (n, b) in self.active.iter().rev() {
            if b.diffs.iter().any(|(s, _)| s == slot) {
                return Some(*n);
            }
        }
        None
    }

    /// Add a pool-active block with one storage write + one event.
    pub fn add_note_block(
        &mut self,
        number: u64,
        slot: Felt,
        value: Felt,
        event: FxEvent,
    ) {
        let blk = self.active.entry(number).or_default();
        blk.diffs.push((slot, value));
        blk.diffs.sort_by_key(|a| a.0.to_bytes_be());
        blk.events.push(event);
        if number > self.head {
            self.head = number;
        }
    }

    /// Fork the tail: every block >= `from` gets new hashes; active blocks at
    /// and above `from` are dropped (the caller re-adds the post-fork ones).
    pub fn fork_tail(&mut self, from: u64) {
        assert!(from > self.l1_accepted, "cannot fork finalized blocks");
        self.fork_from = from;
        self.fork_salt += 1;
        self.active.retain(|n, _| *n < from);
        if self.head >= from {
            self.head = from - 1;
        }
    }
}
