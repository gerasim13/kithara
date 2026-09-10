#![cfg(not(target_arch = "wasm32"))]

use std::collections::HashSet;

use kithara_test_fixtures::{assets, store};
use kithara_test_utils::kithara;

fn entry_path(entry: &kithara_test_fixtures::asset::AssetEntry) -> std::path::PathBuf {
    let root = store::root_from_env().expect("fixture store is configured");
    store::entry_path(
        &store::namespace(&root, store::CACHE_VERSION.trim()),
        entry.id,
        entry.ext,
    )
}

#[kithara::test(native, flash(false))]
fn the_registry_is_not_empty() {
    assert!(
        !assets::MANIFEST.is_empty(),
        "no assets were registered; the build script must fail before this test can",
    );
}

#[kithara::test(native, flash(false))]
fn every_manifest_entry_is_materialized_or_explicitly_unavailable() {
    for entry in assets::MANIFEST {
        let path = entry_path(entry);
        if let Some(reason) = entry.unavailable {
            assert!(
                !reason.is_empty(),
                "asset {} has an empty failure",
                entry.name
            );
            assert!(
                !path.exists(),
                "unavailable asset {} unexpectedly exists at {}",
                entry.name,
                path.display(),
            );
            continue;
        }
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|error| panic!("asset {} at {}: {error}", entry.name, path.display()));
        assert!(!bytes.is_empty(), "asset {} is empty", entry.name);
    }
}

#[kithara::test(native, flash(false))]
fn every_id_is_unique_and_derived_from_the_accessor_name() {
    let mut seen = HashSet::new();
    for entry in assets::MANIFEST {
        assert!(seen.insert(entry.id), "duplicate asset id {}", entry.id);
        assert!(
            entry_path(entry)
                .file_name()
                .is_some_and(|name| name.to_string_lossy().contains(entry.id)),
            "asset {} does not live at its own id",
            entry.name,
        );
    }
}

#[kithara::test(native, flash(false))]
fn the_pilot_asset_is_a_riff_wave_of_the_declared_length() {
    const HEADER_BYTES: usize = 44;
    const FRAMES: usize = 264_600;
    const CHANNELS: usize = 2;
    const SAMPLE_BYTES: usize = 2;

    let asset = assets::sine_wav_a440_6s();
    let bytes = asset.bytes();

    assert_eq!(&bytes[..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");
    assert_eq!(bytes.len(), HEADER_BYTES + FRAMES * CHANNELS * SAMPLE_BYTES);
    assert_eq!(asset.entry().content_type, "audio/wav");
    assert_eq!(asset.path(), Some(entry_path(asset.entry()).as_path()));
}

#[kithara::test(native, flash(false))]
fn the_accessor_serves_what_the_store_holds() {
    let asset = assets::sine_wav_a440_6s();
    let stored = std::fs::read(asset.path().expect("BUG: the pilot asset lives on disk"))
        .expect("read the stored asset");

    assert_eq!(asset.bytes(), stored.as_slice());
}

#[kithara::test(native, flash(false))]
fn the_namespace_uses_the_explicit_cache_revision() {
    let asset = assets::sine_wav_a440_6s();
    let path = asset.path().expect("BUG: the pilot asset lives on disk");
    let namespace = path.parent().expect("an entry lives inside a namespace");
    let fingerprint = namespace
        .file_name()
        .and_then(|name| name.to_str())
        .expect("the namespace is named by the fingerprint");

    assert_eq!(fingerprint, store::CACHE_VERSION.trim());
}

#[kithara::test(native, flash(false))]
fn an_embedded_asset_has_no_file_to_read() {
    let embedded = assets::marked_sine_wav_a440_6s();
    assert_eq!(
        embedded.path(),
        None,
        "an embedded asset must not carry a store path",
    );
}

#[kithara::test(native, flash(false))]
fn an_embedded_asset_carries_the_bytes_that_were_stored() {
    let embedded = assets::marked_sine_wav_a440_6s();
    let stored = assets::MANIFEST
        .iter()
        .find(|entry| entry.id == embedded.entry().id)
        .map(|entry| std::fs::read(entry_path(entry)).expect("read the stored asset"))
        .unwrap_or_else(|| panic!("asset {} is missing from the manifest", embedded.entry().id));

    assert_eq!(embedded.bytes(), stored.as_slice());
}

#[kithara::test(native, flash(false))]
fn the_encoded_pilot_is_an_mpeg_audio_stream() {
    const ID3_TAG: &[u8; 3] = b"ID3";
    const MPEG_SYNC: u8 = 0xFF;

    let asset = assets::sine_mp3_a440_2s();
    let bytes = asset.bytes();

    assert_eq!(asset.entry().content_type, "audio/mpeg");
    assert!(
        bytes.starts_with(ID3_TAG) || bytes.first() == Some(&MPEG_SYNC),
        "expected an MPEG audio stream, got {:?}",
        &bytes[..bytes.len().min(ID3_TAG.len())],
    );
    assert!(bytes.len() > ID3_TAG.len());
}
