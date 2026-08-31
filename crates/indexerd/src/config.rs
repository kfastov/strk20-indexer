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

/// Sepolia profile — every value verified on-chain, see
/// docs/research/live/sepolia-abi-compat.md. The pool deploys at 8_271_125
/// (`CONTRACT_NOT_FOUND` at 8_271_124) and has been upgraded five times; all
/// six classes are field-level compatible with our decoder for every event we
/// consume, so none of them may degrade decoding.
///
/// The sixth class arrived at block 14_339_893 *during* a live run
/// (docs/research/live/live-run-findings.md session 6). Its ABI is identical
/// for all eight consumed events — only an admin event changed
/// (`OpenNoteDepositorBlockSet` → `OpenNoteScreeningPolicySet`) — and its
/// storage layout was verified by discovering a note written under it
/// (session 7), which no ABI diff could establish.
pub const SEPOLIA_POOL: &str =
    "0x0254a6b2997ef52e9f830ce1f543f6b29768295e8d17e2267d672c552cfe0d91";
pub const SEPOLIA_GENESIS_BLOCK: u64 = 8_271_125;
pub const SEPOLIA_RPC_PRIMARY: &str = "https://starknet-sepolia-rpc.publicnode.com";
pub const SEPOLIA_RPC_FALLBACK: &str = "https://rpc.starknet-testnet.lava.build";
pub const SEPOLIA_CLASSES: [&str; 6] = [
    "0x715b22abfb60815623f4127ba64bd2f93613d8a5c1e519841eaab444659d2af",
    "0x30b8c540cf04d8ef0f4db2a9098d9cc0e35e83af1cb3325f5a4f40144b4b30b",
    "0x1a78d2daee64d1da6e7903b32676c92fcc301d4c03f688cd64e731f46033d18",
    "0x67dddd89d80fedadc06b6f160798f94800a4a70164e5a24301cd0d6076b554d",
    "0x56ab118a8a6e38efc93ad758cefe909fee421fa931ce3cf72df624d345623b2",
    "0x7e2bbd7ccc1e68b2695caef70aeb2a3be6cd017b5d5159278ba08f2d8de33f",
];

/// Which chain a run targets. Selects a whole verified profile; every explicit
/// flag still overrides the profile field by field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum Network {
    #[default]
    Mainnet,
    Sepolia,
}

impl Network {
    pub fn profile(self) -> ChainConfig {
        match self {
            Network::Mainnet => ChainConfig::mainnet(),
            Network::Sepolia => ChainConfig::sepolia(),
        }
    }

    pub fn rpc_primary(self) -> &'static str {
        match self {
            Network::Mainnet => MAINNET_RPC_PRIMARY,
            Network::Sepolia => SEPOLIA_RPC_PRIMARY,
        }
    }

    pub fn rpc_fallback(self) -> &'static str {
        match self {
            Network::Mainnet => MAINNET_RPC_FALLBACK,
            Network::Sepolia => SEPOLIA_RPC_FALLBACK,
        }
    }
}

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
            chain_id: strk20_feed::CHAIN_ID_MAINNET.to_owned(),
            pool: Felt::from_hex(MAINNET_POOL).unwrap(),
            genesis_block: MAINNET_GENESIS_BLOCK,
            epoch_size: MAINNET_EPOCH_SIZE,
            decoder_map,
        }
    }

    pub fn sepolia() -> Self {
        let mut decoder_map = HashMap::new();
        for (i, class) in SEPOLIA_CLASSES.iter().enumerate() {
            decoder_map.insert(Felt::from_hex(class).unwrap(), format!("sepolia-v{i}"));
        }
        Self {
            chain_id: strk20_feed::CHAIN_ID_SEPOLIA.to_owned(),
            pool: Felt::from_hex(SEPOLIA_POOL).unwrap(),
            genesis_block: SEPOLIA_GENESIS_BLOCK,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `--network mainnet` must be exactly the zero-flag behaviour that shipped
    /// before the flag existed (U4).
    #[test]
    fn mainnet_profile_is_the_built_in_default() {
        let cfg = Network::default().profile();
        assert_eq!(cfg.chain_id, "SN_MAIN");
        assert_eq!(cfg.pool, Felt::from_hex(MAINNET_POOL).unwrap());
        assert_eq!(cfg.genesis_block, MAINNET_GENESIS_BLOCK);
        assert_eq!(cfg.epoch_size, MAINNET_EPOCH_SIZE);
        assert_eq!(cfg.decoder_map.len(), 2);
        assert_eq!(Network::Mainnet.rpc_primary(), MAINNET_RPC_PRIMARY);
        assert_eq!(Network::Mainnet.rpc_fallback(), MAINNET_RPC_FALLBACK);
    }

    /// Every class the Sepolia pool has ever run must decode: all five are
    /// field-level compatible with our decoder for every event we consume
    /// (docs/research/live/sepolia-abi-compat.md).
    #[test]
    fn sepolia_profile_covers_the_whole_verified_class_history() {
        let cfg = Network::Sepolia.profile();
        assert_eq!(cfg.chain_id, "SN_SEPOLIA");
        assert_eq!(cfg.genesis_block, 8_271_125);
        assert_eq!(cfg.pool, Felt::from_hex(SEPOLIA_POOL).unwrap());
        for class in SEPOLIA_CLASSES {
            assert!(
                cfg.decoder_map
                    .contains_key(&Felt::from_hex(class).unwrap()),
                "class {class} missing from the Sepolia decoder map"
            );
        }
        // Sepolia ran mainnet v1 and v2 too; the profiles must agree on them.
        let mainnet = ChainConfig::mainnet();
        for class in mainnet.decoder_map.keys() {
            assert!(cfg.decoder_map.contains_key(class));
        }
    }
}
