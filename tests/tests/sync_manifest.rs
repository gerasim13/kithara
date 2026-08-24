#![cfg(not(target_arch = "wasm32"))]

use std::collections::BTreeSet;

use kithara_integration_tests::{
    kithara,
    sync_manifest::{OracleState, registrations, write_manifest_from_env},
};

#[kithara::test]
fn sync_oracle_manifest_is_complete_unique_and_exportable() {
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
        108,
        "manifest lost or silently added a frozen product, lower-level, or active oracle row"
    );
    let manifest = write_manifest_from_env().expect("optional sync manifest export succeeds");
    if std::env::var_os("KITHARA_SYNC_MANIFEST_DIR").is_some() {
        assert!(
            manifest.is_some_and(|path| path.is_file()),
            "configured sync manifest export must create its artifact"
        );
    }
}
