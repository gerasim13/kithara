#![cfg(not(target_arch = "wasm32"))]
#![forbid(unsafe_code)]

//! `ResourceConfig::asset_key` mints keys for artifacts derived from a track
//! — the analysis blob kithara-app caches per track. Those artifacts are
//! meant to live under the same asset as the track itself, so evicting or
//! deleting the track takes them with it.
//!
//! Nothing enforces that: play derives the asset root from its own `src` and
//! discriminator, while kithara-file and kithara-hls each derive it again
//! inside `create`. Three derivations, one root, no shared owner. These tests
//! pin the agreement from the outside — they ask the store whether the key
//! play mints names a resource the stream actually wrote, so a drift on any
//! of the three paths surfaces here rather than as a waveform that outlives
//! its track.

use kithara::{
    assets::{AssetResource, AssetResourceState, AssetStore, ResourceKey},
    audio::AudioDecoderConfig,
    decode::DecoderBackend,
    platform::{sync::Arc, time::Duration},
    play::{Resource, ResourceConfig},
};
use kithara_integration_tests::{
    Content, Delivery, FixtureBehavior, TestServerHelper, TestTempDir, create_wav_exact_bytes,
    hls_server::{HlsTestServer, HlsTestServerConfig},
    kithara,
    signal_pcm::signal,
    temp_dir,
};

use crate::common::test_defaults::Consts as Shared;

/// Both tracks are keyed with a discriminator: it is the field most likely to
/// be dropped on one of the three derivation paths, and dropping it moves the
/// root without moving the bytes.
const DISCRIMINATOR: &str = "derived-key-pin";

const SEGMENT_COUNT: usize = 3;
const SEGMENT_SIZE: u32 = 65_536;
const SAMPLE_RATE: u32 = 44_100;
const CHANNELS: u16 = 2;

/// The resource kithara-app caches per track.
fn analysis_resource() -> AssetResource {
    AssetResource::Named {
        namespace: "analysis".to_string(),
        name: "track.analysis".to_string(),
    }
}

/// The source resource kithara-file derives for a remote URL: the extension
/// comes from the URL leaf, which is what `build_file_config` passes as the
/// hint.
fn source_resource() -> AssetResource {
    AssetResource::Source {
        extension: "mp3".to_string(),
    }
}

fn config_for(url: &url::Url, store: AssetStore, discriminator: Option<&str>) -> ResourceConfig {
    ResourceConfig::for_src(url.as_str())
        .expect("track url parses")
        .store(store)
        .maybe_discriminator(discriminator.map(str::to_owned))
        .decoder(
            AudioDecoderConfig::builder()
                .backend(DecoderBackend::Symphonia)
                .build(),
        )
        .byte_pool(kithara::bufpool::BytePool::default())
        .pcm_pool(kithara::bufpool::PcmPool::default())
        .build()
}

/// Assert that `track` names bytes the stream wrote, and that the derived
/// artifact is rooted at the same asset.
fn assert_shares_asset(store: &AssetStore, track: &ResourceKey, derived: &ResourceKey) {
    assert_ne!(
        store
            .resource_state(track)
            .expect("track key resolves in the store"),
        AssetResourceState::Missing,
        "the key play mints for the track names no resource the stream wrote — play and the \
         stream disagree about the track's asset root"
    );
    assert_eq!(
        derived.asset_root(),
        track.asset_root(),
        "a derived artifact must be rooted at the track's asset so it shares its lifecycle"
    );
}

async fn mp3_url() -> (TestServerHelper, url::Url) {
    let helper = TestServerHelper::new().await;
    let track = helper.register_behavior(FixtureBehavior {
        content: Content::StaticBytes {
            bytes: Arc::new(Shared::TEST_MP3_BYTES.to_vec()),
            content_type: Some("audio/mpeg"),
        },
        delivery: Delivery::Range,
    });
    let url = track.child_url("track.mp3");
    (helper, url)
}

async fn hls_server() -> HlsTestServer {
    let segment_size = usize::try_from(SEGMENT_SIZE).expect("segment size fits usize");
    let bytes_per_second = f64::from(SAMPLE_RATE) * f64::from(CHANNELS) * 2.0;
    HlsTestServer::new(HlsTestServerConfig {
        custom_data: Some(Arc::new(create_wav_exact_bytes(
            signal::Sawtooth,
            SAMPLE_RATE,
            CHANNELS,
            SEGMENT_COUNT * segment_size,
        ))),
        segment_duration_secs: f64::from(SEGMENT_SIZE) / bytes_per_second,
        segment_size,
        segments_per_variant: SEGMENT_COUNT,
        // The fixture serves raw WAV under `.m4s` URIs; without the announced
        // codec the coordinator infers fMP4 and waits for an init segment
        // this fixture has none of.
        codecs: Some("wav".to_string()),
        ..Default::default()
    })
    .await
}

#[kithara::test(
    tokio,
    timeout(Duration::from_secs(30)),
    env(KITHARA_HANG_TIMEOUT_SECS = "10")
)]
async fn analysis_blob_is_rooted_at_the_progressive_track(temp_dir: TestTempDir) {
    let (_server, url) = mp3_url().await;
    let store = kithara_integration_tests::disk_asset_store(temp_dir.path());
    let config = config_for(&url, store.clone(), Some(DISCRIMINATOR));

    let track = config
        .asset_key(&source_resource())
        .expect("play mints the track key");
    let analysis = config
        .asset_key(&analysis_resource())
        .expect("play mints the analysis key");

    let _resource = Resource::new(config)
        .await
        .expect("progressive track opens");

    assert_shares_asset(&store, &track, &analysis);
}

#[kithara::test(
    tokio,
    timeout(Duration::from_secs(30)),
    env(KITHARA_HANG_TIMEOUT_SECS = "10")
)]
async fn analysis_blob_is_rooted_at_the_hls_track(temp_dir: TestTempDir) {
    let server = hls_server().await;
    let url = server.url("/master.m3u8");
    let store = kithara_integration_tests::disk_asset_store(temp_dir.path());
    let config = config_for(&url, store.clone(), Some(DISCRIMINATOR));

    // Every HLS resource is keyed by its own URL under the stream's scope,
    // the master playlist included — so this is the key `Hls::create` commits
    // the playlist under.
    let track = config
        .asset_key(&AssetResource::Url(url.clone()))
        .expect("play mints the master key");
    let analysis = config
        .asset_key(&analysis_resource())
        .expect("play mints the analysis key");

    let _resource = Resource::new(config).await.expect("HLS track opens");

    assert_shares_asset(&store, &track, &analysis);
}

/// A discriminator has to reach the stream, not just play's own key minting:
/// two configs that differ only by discriminator must land on two assets, or
/// the second track silently reads the first one's bytes.
#[kithara::test(
    tokio,
    timeout(Duration::from_secs(30)),
    env(KITHARA_HANG_TIMEOUT_SECS = "10")
)]
async fn a_discriminator_moves_the_whole_asset(temp_dir: TestTempDir) {
    let (_server, url) = mp3_url().await;
    let store = kithara_integration_tests::disk_asset_store(temp_dir.path());

    let discriminated = config_for(&url, store.clone(), Some(DISCRIMINATOR));
    let plain = config_for(&url, store.clone(), None);
    let discriminated_key = discriminated
        .asset_key(&source_resource())
        .expect("discriminated key");
    let plain_key = plain.asset_key(&source_resource()).expect("plain key");

    assert_ne!(
        discriminated_key.asset_root(),
        plain_key.asset_root(),
        "a discriminator must select a different asset"
    );

    let _resource = Resource::new(discriminated)
        .await
        .expect("discriminated track opens");

    assert_ne!(
        store
            .resource_state(&discriminated_key)
            .expect("discriminated key resolves"),
        AssetResourceState::Missing,
        "the discriminator reached the stream: the bytes are under the discriminated asset"
    );
    assert_eq!(
        store
            .resource_state(&plain_key)
            .expect("plain key resolves"),
        AssetResourceState::Missing,
        "no bytes landed under the undiscriminated asset"
    );
}
