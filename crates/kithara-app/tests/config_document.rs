//! What the shipped document promises the application, end to end.

use std::fs;

use kithara_app::document::Config;

#[kithara::test(native, flash(false))]
fn the_shipped_document_configures_the_application() {
    let config = Config::load(None, None).expect("the baked document stands alone");

    assert!(
        !config.tracks().is_empty(),
        "the shipped playlist reaches the application"
    );
    assert!(
        config.should_accept_invalid_certs(),
        "the shipped document accepts test-server certificates"
    );
    config
        .drm_policy()
        .expect("the shipped providers are valid");
}

#[kithara::test(native, flash(false))]
fn a_file_changes_the_playlist_without_touching_the_rest() {
    let path = std::env::temp_dir().join("kithara-config-e2e-playlist.yaml");
    fs::write(
        &path,
        "playlist:\n  tracks: [https://example.test/one.mp3]\n",
    )
    .expect("write the test document");

    let config = Config::load(Some(&path), None).expect("the overlay loads");

    assert_eq!(config.tracks(), ["https://example.test/one.mp3"]);
    assert!(
        config.should_accept_invalid_certs(),
        "a field the overlay never names keeps its baked value"
    );
}

#[kithara::test(native, flash(false))]
fn the_app_section_reaches_the_config_patch() {
    let path = std::env::temp_dir().join("kithara-config-e2e-app.yaml");
    fs::write(&path, "app:\n  eq_bands: 5\n  broadcast_tap_lead: 750ms\n")
        .expect("write the test document");

    let config = Config::load(Some(&path), None).expect("the overlay loads");

    let settings = config.app_settings();
    assert_eq!(settings.eq_bands, Some(5));
    assert_eq!(
        settings.broadcast_tap_lead,
        Some(std::time::Duration::from_millis(750))
    );
    assert_eq!(
        settings.waveform_max_buckets, None,
        "a knob the document never names stays unset, so the built value stands"
    );
}

#[kithara::test(native, flash(false))]
fn an_unknown_app_knob_is_refused() {
    let path = std::env::temp_dir().join("kithara-config-e2e-unknown.yaml");
    fs::write(&path, "app:\n  eq_band: 5\n").expect("write the test document");

    let error = Config::load(Some(&path), None).expect_err("a typo must not pass silently");

    assert!(error.to_string().contains("eq_band"), "{error}");
}
