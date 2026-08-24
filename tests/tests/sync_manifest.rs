#![cfg(not(target_arch = "wasm32"))]

use std::collections::BTreeSet;

use kithara_integration_tests::{
    kithara,
    sync_manifest::{ActivationWave, OracleState, registrations},
};

#[kithara::test]
fn sync_oracle_manifest_is_complete_and_unique() {
    let rows = registrations();
    let ids = rows
        .iter()
        .map(|row| row.oracle_id())
        .collect::<BTreeSet<_>>();
    let blocked_product = rows
        .iter()
        .filter(|row| row.state() == OracleState::BlockedProduct)
        .count();
    let blocked_fixture = rows
        .iter()
        .filter(|row| row.state() == OracleState::BlockedFixture)
        .count();

    assert_eq!(ids.len(), rows.len(), "oracle IDs must be unique");
    assert!(
        rows.iter().all(|row| row.has_complete_provenance()),
        "every oracle row must retain source, contract, and destination provenance"
    );
    for wave in [
        ActivationWave::Foundation,
        ActivationWave::ResidentPlan,
        ActivationWave::QueueAdapter,
        ActivationWave::AppToggle,
        ActivationWave::Acceptance,
    ] {
        assert!(
            rows.iter().any(|row| row.activation_wave() == wave),
            "oracle registry must retain the {wave:?} activation wave"
        );
    }
    assert_eq!(
        blocked_product, 66,
        "all non-library product rows and legacy renderer mappings must stay registered"
    );
    assert_eq!(
        blocked_fixture, 11,
        "the full opt-in library matrix must stay registered separately"
    );
    assert_eq!(
        rows.len(),
        135,
        "manifest lost or silently added a frozen product, lower-level, or active oracle row"
    );
}
