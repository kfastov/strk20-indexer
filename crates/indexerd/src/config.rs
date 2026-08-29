//! Chain configuration. Mainnet defaults are built in (U4: `strk20 run` with
//! zero flags on a mainnet box must work); tests override every field.

use starknet_types_core::felt::Felt;
use std::collections::HashMap;

pub const MAINNET_POOL: &str =
    "0x040337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a";
pub const MAINNET_GENESIS_BLOCK: u64 = 8_978_970;
pub const MAINNET_EPOCH_SIZE: u64 = 10_000;
pub const MAINNET_RPC_PRIMARY: &str = "https://rpc.starknet.lava.build";
pub const MAINNET_RPC_FALLBACK: &str = "https://starknet.publicnode.com";
/// Verified on-chain (docs/research/q1-version-pin.md).
pub const MAINNET_CLASS_V1: &str =
    "0x30b8c540cf04d8ef0f4db2a9098d9cc0e35e83af1cb3325f5a4f40144b4b30b";
pub const MAINNET_CLASS_V2: &str =
    "0x67dddd89d80fedadc06b6f160798f94800a4a70164e5a24301cd0d6076b554d";

#[derive(Debug, Clone)]
pub struct ChainConfig {
    pub chain_id: String,
    pub pool: Felt,
    pub genesis_block: u64,
    pub epoch_size: u64,
    /// class hash -> decoder version name; an on-chain class outside this map
    /// switches typed decoding into degraded mode (spec §5.7).
    pub decoder_map: HashMap<Felt, String>,
}

impl ChainConfig {
    pub fn mainnet() -> Self {
        let mut decoder_map = HashMap::new();
        decoder_map.insert(Felt::from_hex(MAINNET_CLASS_V1).unwrap(), "v1".to_owned());
        decoder_map.insert(Felt::from_hex(MAINNET_CLASS_V2).unwrap(), "v2".to_owned());
        Self {
            chain_id: "SN_MAIN".to_owned(),
            pool: Felt::from_hex(MAINNET_POOL).unwrap(),
            genesis_block: MAINNET_GENESIS_BLOCK,
            epoch_size: MAINNET_EPOCH_SIZE,
            decoder_map,
        }
    }

    /// Epoch index covering `block`.
    pub fn epoch_of(&self, block: u64) -> u64 {
        block / self.epoch_size
    }

    /// Inclusive block range of epoch `idx` (absolute alignment, spec §4.2).
    pub fn epoch_range(&self, idx: u64) -> (u64, u64) {
        (idx * self.epoch_size, (idx + 1) * self.epoch_size - 1)
    }

    /// First epoch that can ever contain pool data.
    pub fn first_epoch(&self) -> u64 {
        self.epoch_of(self.genesis_block)
    }
}
