use std::num::NonZeroU32;

use kithara_assets::{AssetLayoutRegistry, AssetScope, AssetSource, AssetStore};
use kithara_audio::{AssetAxis, Beat, BeatMap, MapAxis, MapPoint};
use kithara_bufpool::{BytePool, PcmPool};
use kithara_platform::sync::Arc;
use kithara_play::{
    AssetMapRegistry, AssetMapRegistryError, PlaybackDirection, ResourceConfig, SessionBeat,
    SyncUnavailable, TrackBinding,
    policy::{QueryIdentityLayout, QueryIdentityRule},
};
use kithara_test_utils::kithara;
use url::Url;

struct TestAsset;

fn source(url: &str) -> AssetSource {
    AssetSource::Remote {
        url: Url::parse(url).expect("invariant: fixture URL is valid"),
        discriminator: None,
    }
}

fn scope(store: &AssetStore, url: &str) -> AssetScope {
    store
        .scope::<TestAsset>(&source(url))
        .expect("invariant: fixture source has a valid layout scope")
}

fn axis(frame_count: u64) -> AssetAxis {
    AssetAxis::new(
        NonZeroU32::new(48_000).expect("invariant: fixture sample rate is non-zero"),
        frame_count,
    )
}

#[kithara::test]
fn equal_asset_sources_share_one_map_owner() {
    let store = AssetStore::builder().build();
    let registry = AssetMapRegistry::default();
    let source = scope(&store, "https://example.com/track.wav");
    let first = registry
        .map(&source, axis(96_000))
        .expect("invariant: first registration is valid");
    let second = first.map();

    assert_eq!(first.id(), second.id());
}

#[kithara::test]
fn independent_registries_cannot_alias_stamps_or_accept_foreign_points() {
    let store = AssetStore::builder().build();
    let source = scope(&store, "https://example.com/track.wav");
    let first = AssetMapRegistry::default()
        .map(&source, axis(96_000))
        .expect("invariant: first registry can allocate a map");
    let second = AssetMapRegistry::default()
        .map(&source, axis(96_000))
        .expect("invariant: second registry can allocate a map");
    let first_snapshot = first.snapshot();
    let second_snapshot = second.snapshot();

    assert_ne!(first_snapshot.stamp(), second_snapshot.stamp());
    let foreign = MapPoint::new(
        second_snapshot.stamp(),
        Beat::new(0.0).expect("invariant: fixture beat is finite"),
    );
    let result = TrackBinding::new(
        first_snapshot.clone(),
        SessionBeat::new(0.0).expect("invariant: fixture session beat is finite"),
        foreign,
        PlaybackDirection::Forward,
    );

    assert!(matches!(
        result,
        Err(SyncUnavailable::StaleAnchor { expected, given })
            if expected == first_snapshot.stamp() && given == second_snapshot.stamp()
    ));
}

#[kithara::test]
fn public_resource_scope_feeds_the_registry_identity_contract() {
    let store = AssetStore::builder().build();
    let src = ResourceConfig::parse_src("https://example.com/track.flac?token=one")
        .expect("invariant: fixture source is valid");
    let config: ResourceConfig = ResourceConfig::for_src(src)
        .store(store)
        .byte_pool(BytePool::default())
        .pcm_pool(PcmPool::default())
        .build();
    let scope = config
        .asset_scope()
        .expect("invariant: public playback identity is valid");
    let registration = AssetMapRegistry::default()
        .map(&scope, axis(96_000))
        .expect("invariant: canonical playback scope can register a map");

    assert_eq!(registration.snapshot().axis(), MapAxis::Asset(axis(96_000)));
}

#[kithara::test]
fn different_asset_sources_receive_different_map_ids() {
    let store = AssetStore::builder().build();
    let registry = AssetMapRegistry::default();
    let first = registry
        .map(
            &scope(&store, "https://example.com/first.wav"),
            axis(96_000),
        )
        .expect("invariant: first registration is valid");
    let second = registry
        .map(
            &scope(&store, "https://example.com/second.wav"),
            axis(96_000),
        )
        .expect("invariant: second registration is valid");

    assert_ne!(first.id(), second.id());
}

#[kithara::test]
fn refreshed_signed_url_reuses_the_canonical_asset_map() {
    let store = AssetStore::builder().build();
    let registry = AssetMapRegistry::default();
    let first = scope(&store, "https://example.com/track.wav?token=one");
    let second = scope(&store, "https://example.com/track.wav?token=two#fragment");

    let first = registry
        .map(&first, axis(96_000))
        .expect("invariant: first signed source is valid");
    assert!(matches!(
        registry.map(&second, axis(96_000)),
        Err(AssetMapRegistryError::PublisherClaimed)
    ));
    drop(first);
}

#[kithara::test]
fn configured_query_identity_separates_tracks_without_splitting_renewed_urls() {
    let layout = Arc::new(QueryIdentityLayout::new([QueryIdentityRule::new(
        ["example.com"],
        ["track_id"],
    )]));
    let layouts = AssetLayoutRegistry::default().with::<TestAsset>(layout);
    let store = AssetStore::builder().layouts(layouts).build();
    let registry = AssetMapRegistry::default();
    let first = scope(&store, "https://example.com/play?track_id=one&token=first");
    let renewed = scope(&store, "https://example.com/play?token=second&track_id=one");
    let different = scope(&store, "https://example.com/play?track_id=two&token=first");

    let first = registry
        .map(&first, axis(96_000))
        .expect("invariant: first layout identity is valid");
    assert!(matches!(
        registry.map(&renewed, axis(96_000)),
        Err(AssetMapRegistryError::PublisherClaimed)
    ));
    let different = registry
        .map(&different, axis(96_000))
        .expect("invariant: different layout identity is valid");

    assert_ne!(first.id(), different.id());
}

#[kithara::test]
fn equal_source_with_conflicting_axis_is_rejected() {
    let store = AssetStore::builder().build();
    let registry = AssetMapRegistry::default();
    let source = scope(&store, "https://example.com/track.wav");
    let _registration = registry
        .map(&source, axis(96_000))
        .expect("invariant: first registration is valid");

    assert!(matches!(
        registry.map(&source, axis(48_000)),
        Err(AssetMapRegistryError::AxisConflict { .. })
    ));
}
