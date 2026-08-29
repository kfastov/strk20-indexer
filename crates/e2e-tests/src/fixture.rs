//! Vendored loader for upstream's engine-level devnet fixture
//! (fixtures/upstream/devnet-state.json @ CONTRACT_V2_DEPLOYED_MAINNET_2026-07-08,
//! Apache-2.0). Upstream keeps its loader #[cfg(test)]-private, so this is a
//! (slightly simplified) copy: viewing keys are plain Felt here and wrapped
//! into SecretFelt at use sites.

use serde::Deserialize;
use starknet_types_core::felt::Felt;
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize)]
pub struct DevnetConstants {
    pub contract_address: Felt,
    pub alice_address: Felt,
    pub alice_viewing_key: Felt,
    pub bob_address: Felt,
    pub bob_viewing_key: Felt,
    pub admin_address: Felt,
    pub eth_token: Felt,
    pub strk_token: Felt,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DevnetFixture {
    pub constants: DevnetConstants,
    #[serde(default)]
    pub block: u64,
    pub slots: HashMap<Felt, Felt>,
}

pub fn load_devnet_fixture() -> DevnetFixture {
    const JSON: &str = include_str!("../../../fixtures/upstream/devnet-state.json");
    serde_json::from_str(JSON).expect("parse devnet fixture")
}
