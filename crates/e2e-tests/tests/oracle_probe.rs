//! Conformance evidence: the upstream devnet fixture's single bob note is
//! SPENT, and the engine filters spent notes — 1 channel, 0 notes is the
//! CORRECT reference result (upstream's own step-5 test asserts the same).

use discovery_core::privacy_pool::types::SecretFelt;
use discovery_core::storage_backend::MockBackend;
use e2e_tests::fixture::load_devnet_fixture;
use e2e_tests::oracle;

#[tokio::test(flavor = "multi_thread")]
async fn fixture_bob_note_is_spent_and_filtered() {
    let f = load_devnet_fixture();
    let backend = MockBackend::new(f.slots.clone());
    let r = oracle::incoming(
        &backend,
        f.constants.bob_address,
        &SecretFelt::new(f.constants.bob_viewing_key),
    )
    .await;
    assert_eq!(r.cursor.channels.len(), 1, "bob has one incoming channel (from alice)");
    assert!(r.notes.is_empty(), "the fixture note is spent -> filtered");
    assert!(r.cursor.is_complete());
}
