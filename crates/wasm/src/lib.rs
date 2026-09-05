//! Pure WASM facade: stage public bytes, verify a checkpoint, discover locally.
//! The host runs this module in a Worker. Local folded caches include private
//! discovery cursors and are trusted; no key-bearing value is sent to the feed.
#![deny(unsafe_code)]
pub mod blob;
pub mod drive;
mod err;
pub mod staged;
pub use engine::Engine;

#[allow(unsafe_code)]
mod engine {
    use super::{blob, drive::drive, err::to_js, staged::StagedFeed};
    use anyhow::{anyhow, ensure, Result};
    use discovery_core::privacy_pool::types::SecretFelt;
    use serde_json::json;
    use starknet_types_core::felt::Felt;
    use strk20_consumer::{
        mem::MemStore,
        sdk,
        store::{ColdStart, ConsumerStore},
    };
    use strk20_feed::{
        checkpoint::{verify_checkpoint, TrustedCheckpoint},
        manifest::Genesis,
    };
    use wasm_bindgen::prelude::*;
    use zeroize::Zeroize;

    #[wasm_bindgen]
    pub struct Engine {
        store: MemStore,
        feed: StagedFeed,
        genesis: Genesis,
        pending: Option<(TrustedCheckpoint, Felt)>,
        failed: bool,
    }

    fn number(store: &MemStore, name: &str) -> u64 {
        store
            .meta_get(name)
            .ok()
            .flatten()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    }
    fn felt(s: &str) -> Result<Felt> {
        Ok(strk20_feed::felt_from_hex(s)?)
    }
    fn secret(bytes: &mut [u8]) -> Result<SecretFelt> {
        let result = if bytes.len() == 32 {
            let value = Felt::from_bytes_be(&bytes.try_into().unwrap());
            if value == Felt::ZERO {
                Err(anyhow!("KEY_INVALID: zero viewing key"))
            } else {
                Ok(SecretFelt::new(value))
            }
        } else {
            Err(anyhow!("KEY_INVALID: expected 32 bytes"))
        };
        bytes.zeroize();
        result
    }

    #[wasm_bindgen]
    impl Engine {
        #[wasm_bindgen(constructor)]
        pub fn new(genesis_json: &str) -> Result<Self, JsError> {
            to_js(Self::build(genesis_json))
        }
        fn build(genesis_json: &str) -> Result<Self> {
            let feed = StagedFeed::new();
            let genesis = feed.set_genesis(genesis_json)?;
            Ok(Self {
                feed,
                genesis,
                store: MemStore::new(),
                pending: None,
                failed: false,
            })
        }
        pub fn version() -> String {
            blob::ENGINE_VERSION.into()
        }
        pub fn stage_manifest(&self, json: &str) -> Result<(), JsError> {
            to_js(self.feed.set_manifest(json))
        }
        pub fn stage_epoch(&self, epoch: u64, payload: &[u8]) {
            self.feed.put_epoch(epoch, payload.to_vec());
        }
        pub fn stage_snapshot(&self, epoch: u64, compressed: &[u8], payload: &[u8]) {
            self.feed
                .put_snapshot(epoch, compressed.to_vec(), payload.to_vec());
        }
        pub fn stage_head(&self, payload: &[u8], etag: &str) {
            self.feed.put_head(payload.to_vec(), etag.to_owned());
        }

        /// The host obtains this header independently, never from the feed or
        /// the proof being verified. This is the cheap path before cold folding.
        pub fn stage_checkpoint(
            &mut self,
            checkpoint_json: &str,
            proof_json: &str,
        ) -> Result<(), JsError> {
            self.failed = true;
            self.pending = None;
            to_js((|| {
                let cp: TrustedCheckpoint = serde_json::from_str(checkpoint_json)?;
                ensure!(
                    cp.chain_id == self.genesis.chain_id && cp.pool == felt(&self.genesis.pool)?,
                    "CHAIN_MISMATCH: checkpoint identity"
                );
                let root = verify_checkpoint(&cp, proof_json)?;
                self.pending = Some((cp, root));
                Ok(())
            })())
        }

        /// Work on a candidate copy. Neither malformed diffs nor a failed root
        /// comparison can replace the last successfully verified cache.
        pub fn apply(&mut self, cold_start: &str) -> Result<String, JsError> {
            self.failed = true;
            to_js((|| {
                let (cp, root) = self
                    .pending
                    .as_ref()
                    .ok_or_else(|| anyhow!("CHECKPOINT_REQUIRED: stage an independent proof"))?;
                let manifest = self
                    .feed
                    .manifest()
                    .ok_or_else(|| anyhow!("NOT_STAGED: manifest"))?;
                if self.store.is_empty()? {
                    if let Some(s) = &manifest.snapshot {
                        ensure!(
                            cold_start == "epochs" || s.block <= cp.block_number,
                            "BOUND_BELOW_SNAPSHOT: snapshot is newer than checkpoint"
                        );
                    }
                }
                let mode = match cold_start {
                    "epochs" => ColdStart::Epochs,
                    "snapshot" => ColdStart::Snapshot,
                    "auto" => ColdStart::Auto,
                    _ => return Err(anyhow!("CONFIG_INVALID: cold start mode")),
                };
                let candidate = self.store.fork();
                let outcome = drive(strk20_consumer::apply::apply_feed(
                    &candidate, &self.feed, mode,
                ))?;
                strk20_consumer::anchors::verify_state(&candidate, cp, *root)?;
                candidate.meta_set("applied_manifest", &serde_json::to_string(&manifest)?)?;
                self.store = candidate;
                self.failed = false;
                self.feed.clear_applied();
                Ok(json!({"head":outcome.head,"verifiedAt":cp.block_number,"tail_rewound":outcome.tail_rewound,
                    "epochs_applied":outcome.epochs_applied,"snapshot_basis":outcome.snapshot_basis}).to_string())
            })())
        }

        pub fn info(&self) -> Result<String, JsError> {
            to_js((|| {
                let cp = self
                    .store
                    .meta_get("verified_checkpoint")?
                    .map(|s| serde_json::from_str::<TrustedCheckpoint>(&s))
                    .transpose()?;
                Ok(json!({"chain_id":self.genesis.chain_id,"pool":self.genesis.pool,
                    "head":number(&self.store,"head_number"),"last_epoch":self.store.meta_get("last_epoch_applied")?.and_then(|s|s.parse::<u64>().ok()),
                    "last_epoch_to":number(&self.store,"last_epoch_to"),"history_floor":number(&self.store,"history_floor"),
                    "snapshot_basis":self.store.meta_get("snapshot_basis")?.and_then(|s|s.parse::<u64>().ok()),
                    "verifiedAt":cp.as_ref().map(|cp|cp.block_number),"checkpoint":cp,
                    "verificationFailed":self.failed,"verified":if self.failed {"failed"} else if cp.is_some(){"rpc-verified"}else{"unverified"},
                    "engine_version":blob::ENGINE_VERSION}).to_string())
            })())
        }
        pub fn export_state(&self) -> Result<Vec<u8>, JsError> {
            to_js((|| {
                sdk::checkpoint(&self.store)?;
                // The saved manifest comes from the successfully applied store,
                // not a subsequently staged candidate.
                let manifest = self
                    .store
                    .meta_get("applied_manifest")?
                    .ok_or_else(|| anyhow!("missing applied manifest"))?;
                blob::encode(&self.store, &serde_json::from_str(&manifest)?)
            })())
        }
        pub fn load(bytes: &[u8], genesis_json: &str) -> Result<Self, JsError> {
            to_js((|| {
                let mut engine = Self::build(genesis_json)?;
                let (store, manifest) = blob::decode(bytes, &engine.genesis)?;
                engine
                    .feed
                    .set_manifest(&serde_json::to_string(&manifest)?)?;
                engine.store = store;
                sdk::checkpoint(&engine.store)?;
                Ok(engine)
            })())
        }
        pub fn discover(&self, owner: &str, key: &mut [u8]) -> Result<String, JsError> {
            let key = secret(key);
            to_js((|| {
                ensure!(!self.failed, "CHECKPOINT_FAILED: sync must succeed first");
                drive(sdk::discover(&self.store, felt(owner)?, &key?))
            })())
        }
        pub fn channels(
            &self,
            owner: &str,
            key: &mut [u8],
            recipients_json: &str,
        ) -> Result<String, JsError> {
            let key = secret(key);
            to_js((|| {
                ensure!(!self.failed, "CHECKPOINT_FAILED: sync must succeed first");
                let recipients: Option<Vec<String>> = serde_json::from_str(recipients_json)?;
                let recipients = recipients
                    .map(|rs| rs.iter().map(|s| felt(s)).collect::<Result<Vec<_>>>())
                    .transpose()?;
                Ok(sdk::channels(&self.store, felt(owner)?, &key?, recipients)?.to_string())
            })())
        }
        pub fn requirement(
            &self,
            owner: &str,
            key: &mut [u8],
            recipient: &str,
            token: &str,
        ) -> Result<u8, JsError> {
            let key = secret(key);
            to_js((|| {
                ensure!(!self.failed, "CHECKPOINT_FAILED: sync must succeed first");
                sdk::requirement(
                    &self.store,
                    felt(owner)?,
                    &key?,
                    felt(recipient)?,
                    felt(token)?,
                )
            })())
        }
        pub fn forget_owner(&self, owner: &str) -> Result<(), JsError> {
            to_js(strk20_consumer::sync::full_resync(
                &self.store,
                &to_js(felt(owner))?,
            ))
        }
    }
}
