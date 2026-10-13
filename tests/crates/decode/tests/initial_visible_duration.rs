use std::{io::Cursor, num::NonZeroU32};

use ::kithara::{
    assets::{AssetStore, StorageBackend},
    audio::{Audio, AudioConfig, ChunkOutcome, SeekOutcome},
    decode::{DecoderChunkOutcome, DecoderConfig, DecoderFactory},
    file::{File, FileConfig},
    platform::{sync::Arc, time::Duration, tokio::task::yield_now},
    resampler::NoResamplerBackend,
    signal::AudioSpec,
    stream::mock::NoopWorkerWake,
};
use kithara_integration_tests::{
    TestServerHelper,
    bufpool_ext::{TestPools, pools},
};
use kithara_test_fixtures::SignalAsset;
use kithara_test_utils::kithara;

#[kithara::test(native, tokio, timeout(Duration::from_secs(10)), hang_timeout_secs(1))]
#[kithara::hang_watchdog(ctx = String)]
#[case(SignalAsset::PROFILE_MP3_LIBMP3LAME_11025_1CH)]
#[case(SignalAsset::PROFILE_MP3_LIBMP3LAME_22050_1CH)]
#[case(SignalAsset::PROFILE_MP3_LIBMP3LAME_44100_1CH)]
#[case(SignalAsset::PROFILE_MP3_LIBMP3LAME_44100_2CH)]
#[case(SignalAsset::PROFILE_M4A_AAC_44100_2CH)]
#[case(SignalAsset::PROFILE_TAGGED_MP3_ID3)]
#[case(SignalAsset::PROFILE_TAGGED_WAVE_MP3_ID3)]
async fn initial_visible_duration_matches_the_built_ruler_before_eof(#[case] asset: SignalAsset) {
    let server = TestServerHelper::new().await;
    let url = server.signal(asset);
    let body = reqwest::get(url.clone())
        .await
        .expect("encoded fixture response")
        .error_for_status()
        .expect("encoded fixture status")
        .bytes()
        .await
        .expect("encoded fixture bytes");
    let pools = pools();
    let mut decoder = DecoderFactory::create_with_probe(
        Cursor::new(body.to_vec()),
        Some(asset.ext()),
        DecoderConfig::<NoResamplerBackend, TestPools>::builder()
            .pools(pools.clone())
            .build(),
    )
    .expect("initial encoded decoder");
    let raw_before = decoder.duration();
    let profile_before = decoder.gapless_profile(None);
    let origin_before = decoder.initial_pcm_position();
    let DecoderChunkOutcome::Chunk(first) = decoder.next_chunk().expect("original decoder PCM")
    else {
        panic!("in-memory encoded source must produce original PCM");
    };
    assert!(first.frames() > 0);
    assert_eq!(
        first.samples.len(),
        first.frames() * usize::from(first.spec().channels)
    );
    assert!(first.samples.iter().all(|sample| sample.is_finite()));
    let raw_after = decoder.duration();
    let profile_after = decoder.gapless_profile(None);
    let origin_after = decoder.initial_pcm_position();
    tracing::info!(asset = asset.name(), ?raw_before, ?raw_after, ?profile_before, ?profile_after,
        ?origin_before, ?origin_after, first_meta = ?first.meta,
        first_frames = first.frames(), first_samples = ?&first.samples[..first.samples.len().min(8)],
        "initial encoded duration owner receipt");

    let file = FileConfig::for_src(url.into())
        .store(
            AssetStore::builder(pools.clone())
                .backend(StorageBackend::Memory)
                .build(),
        )
        .pools(pools.clone())
        .build();
    let config = AudioConfig::<File<TestPools>, NoResamplerBackend>::for_stream(file)
        .hint(asset.ext().to_owned())
        .build();
    let mut audio = Audio::prepare(config, Arc::new(NoopWorkerWake), pools)
        .await
        .expect("audio owner");
    let initial_length = audio
        .duration()
        .expect("initial known visible duration before EOF");
    let initial_frames = AudioSpec::new(2, NonZeroU32::new(44_100).expect("comparison rate"))
        .frame_at(initial_length)
        .expect("visible duration fits the comparison clock");
    tracing::info!(
        asset = asset.name(),
        ?initial_length,
        initial_frames,
        "initial Audio duration receipt"
    );
    assert!(
        initial_frames.abs_diff(264_600) <= 2_048,
        "{} initial length {initial_frames} must match built six-second ruler 264600 before EOF; raw_before={raw_before:?}, raw_after={raw_after:?}, profile_before={profile_before:?}, profile_after={profile_after:?}",
        asset.name()
    );
    let visible = loop {
        hang_tick!(format!("{} first original Audio PCM", asset.name()));
        match audio.next_chunk().expect("visible original PCM") {
            ChunkOutcome::Chunk(chunk) => break chunk,
            ChunkOutcome::Pending { .. } => yield_now().await,
            ChunkOutcome::Eof { .. } => {
                panic!("first output cannot be EOF for the six-second source")
            }
        }
    };
    assert!(visible.frames() > 0);
    assert!(
        !visible.meta.end_of_track,
        "initial length cannot be an EOF refinement"
    );
    assert_eq!(
        visible.samples.len(),
        visible.frames() * usize::from(visible.spec().channels)
    );
    assert!(visible.samples.iter().all(|sample| sample.is_finite()));
    let playhead = audio.playhead_read();
    assert_eq!(visible.meta.visible_origin, Some(visible.meta.timestamp));
    assert_eq!(playhead.duration(), Some(initial_length));
    let target = visible.meta.timestamp + Duration::from_secs(5);
    let sought = audio
        .seek(target)
        .expect("first nonzero raw seek after original output");
    assert!(
        matches!(sought, SeekOutcome::Landed { target: asked, landed_at }
        if asked == target && landed_at <= target)
    );
    let sought = loop {
        hang_tick!(format!("{} first sought Audio PCM", asset.name()));
        match audio.next_chunk().expect("visible sought PCM") {
            ChunkOutcome::Chunk(chunk) => break chunk,
            ChunkOutcome::Pending { .. } => yield_now().await,
            ChunkOutcome::Eof { .. } => panic!("one-second tail cannot be empty"),
        }
    };
    assert_eq!(sought.meta.visible_origin, visible.meta.visible_origin);
    tracing::info!(asset = asset.name(), visible_meta = ?visible.meta, sought_meta = ?sought.meta,
        visible_frames = visible.frames(), visible_samples = ?&visible.samples[..visible.samples.len().min(8)],
        "original visible PCM receipt");
}

#[kithara::test(native, tokio, timeout(Duration::from_secs(10)), hang_timeout_secs(1))]
#[kithara::hang_watchdog(ctx = String)]
#[case::mp3(SignalAsset::PROFILE_MP3_LIBMP3LAME_44100_2CH)]
#[case::aac(SignalAsset::PROFILE_M4A_AAC_44100_2CH)]
async fn preloaded_original_pcm_preserves_its_origin_through_the_first_raw_seek(
    #[case] asset: SignalAsset,
) {
    let server = TestServerHelper::new().await;
    let pools = pools();
    let file = FileConfig::for_src(server.signal(asset).into())
        .store(
            AssetStore::builder(pools.clone())
                .backend(StorageBackend::Memory)
                .build(),
        )
        .pools(pools.clone())
        .build();
    let config = AudioConfig::<File<TestPools>, NoResamplerBackend>::for_stream(file)
        .hint(asset.ext().to_owned())
        .build();
    let mut audio = Audio::prepare(config, Arc::new(NoopWorkerWake), pools)
        .await
        .expect("source owner");
    let reference_file = FileConfig::for_src(server.signal(asset).into())
        .store(
            AssetStore::builder(kithara_integration_tests::bufpool_ext::pools())
                .backend(StorageBackend::Memory)
                .build(),
        )
        .pools(kithara_integration_tests::bufpool_ext::pools())
        .build();
    let reference_config =
        AudioConfig::<File<TestPools>, NoResamplerBackend>::for_stream(reference_file)
            .hint(asset.ext().to_owned())
            .build();
    let mut reference = Audio::prepare(
        reference_config,
        Arc::new(NoopWorkerWake),
        kithara_integration_tests::bufpool_ext::pools(),
    )
    .await
    .expect("independent original source");
    let origin = loop {
        hang_tick!(format!("{} reference original PCM", asset.name()));
        match reference.next_chunk().expect("reference original PCM") {
            ChunkOutcome::Chunk(chunk) => {
                break chunk.meta.visible_origin.expect("original geometry");
            }
            ChunkOutcome::Pending { .. } => yield_now().await,
            ChunkOutcome::Eof { .. } => panic!("six-second reference is not empty"),
        }
    };
    loop {
        hang_tick!(format!("{} original preload", asset.name()));
        audio.preload().expect("preload the real source");
        if audio.is_preloaded() {
            hang_reset!();
            break;
        }
        yield_now().await;
    }
    assert!(audio.is_preloaded());
    let target = origin + Duration::from_secs(1);
    assert!(matches!(audio.seek(target).expect("first raw seek"),
        SeekOutcome::Landed { target: asked, landed_at } if asked == target && landed_at <= target));
    let mut served = 0_u64;
    let mut endpoint = None;
    loop {
        hang_tick!(format!("{} sought PCM frames={served}", asset.name()));
        match audio.next_chunk().expect("real sought PCM") {
            ChunkOutcome::Chunk(chunk) => {
                assert_eq!(
                    chunk.samples.len(),
                    chunk.frames() * usize::from(chunk.spec().channels)
                );
                assert!(chunk.samples.iter().all(|sample| sample.is_finite()));
                assert_eq!(chunk.meta.visible_origin, Some(origin));
                let start = chunk.meta.source_span.map_or(chunk.meta.timestamp, |span| {
                    span.position_at(0).expect("mapped start")
                });
                if let Some(previous) = endpoint {
                    assert_eq!(start, previous);
                } else {
                    assert!(
                        start + chunk.spec().duration_for(1).expect("one source frame") >= target
                    );
                }
                endpoint = Some(chunk.meta.source_span.map_or_else(
                    || {
                        chunk.meta.timestamp
                            + chunk
                                .spec()
                                .duration_for(u64::from(chunk.meta.frames))
                                .expect("actual extent")
                    },
                    |span| {
                        span.position_at(u64::from(chunk.meta.frames))
                            .expect("mapped endpoint")
                    },
                ));
                served += u64::from(chunk.meta.frames);
                hang_reset!();
            }
            ChunkOutcome::Pending { .. } => {}
            ChunkOutcome::Eof { .. } => break,
        }
        yield_now().await;
    }
    assert!(served > 0);
    let endpoint = endpoint.expect("sought suffix emitted PCM");
    assert_eq!(audio.duration(), Some(endpoint - origin));
}
