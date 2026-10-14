use std::num::NonZeroU32;

use kithara::{
    abr::AbrEvent,
    assets::{AssetStore, StorageBackend},
    audio::{AudioConfig, AudioControl, AudioRead, AudioSession, ChunkOutcome, ReadOutcome},
    decode::DecoderBackend,
    download::{Downloader, DownloaderConfig},
    events::EventBus,
    hls::{AbrMode, Hls, HlsConfig},
    host::{HostConfig, HostSettings},
    net::{HttpClient, NetOptions},
    platform::{
        CancelToken,
        thread::paced_backoff,
        time,
        time::{Duration, Instant},
        tokio::task::spawn_blocking,
    },
    play::{PlayWorker, PlayWorkerConfig, RegisteredAudio},
    stream::Stream,
};
use kithara_integration_tests::{
    TestServerHelper, abr_switch_trigger, auto,
    bufpool_ext::{TestPools, pools},
    event::TestEvent,
    fixture_protocol::DelayRule,
    mixed_codec_ladder, mixed_encrypted, mixed_plain,
    offline::{OfflinePlayer, resource_from_reader},
    output_continuity::{
        CONTINUITY_BLOCK_FRAMES, CONTINUITY_SAMPLE_RATE, PlaybackProgressProbe,
        render_offline_window, render_until_audible,
    },
    served_mp3,
};
use kithara_test_utils::{TestTempDir, temp_dir};
use packaged::create_packaged_abr_fixture;
use url::Url;

use super::abr_playback::{
    packaged::{self, open_packaged_hls_audio, packaged_switch_abr_mode, read_audio_some},
    seek::{self, drain_abr_decisions, lowest_pending_estimate},
};

#[kithara::test(
    tokio,
    native,
    serial,
    timeout(Duration::from_secs(30)),
    hang_timeout_secs(3)
)]
async fn abr_switch_on_production_ladder_does_not_hang(
    temp_dir: TestTempDir,
    #[future(awt)] mixed_plain: (TestServerHelper, Url),
) {
    packaged::abr_switch_on_production_ladder_does_not_hang(temp_dir, mixed_plain).await;
}

/// A packaged ABR variant switch leaves no gap in the rendered PCM.
///
/// The seam window reads its own output back, so it renders over a reader that
/// parks on an empty producer ring: a zero-filled underrun would otherwise
/// arrive as silence the decoder never produced, and the measurement would read
/// how promptly the machine scheduled the decode thread instead of what the
/// switch did to the signal. With the park in place a silent block can only be
/// decoded silence, so the run has no tolerance to spend.
#[kithara::test(
    tokio,
    native,
    serial,
    timeout(Duration::from_secs(30)),
    hang_timeout_secs(3),
    tracing("kithara_abr=debug,kithara_audio=debug,kithara_hls=debug,kithara_stream=debug")
)]
async fn packaged_abr_switch_keeps_player_continuity(
    temp_dir: TestTempDir,
    #[future(awt)] create_packaged_abr_fixture: (TestServerHelper, Url),
) {
    let (_server, url) = create_packaged_abr_fixture;
    let pools = pools();
    let worker = PlayWorker::new(PlayWorkerConfig::builder(pools.clone()).build());
    let store = AssetStore::builder(pools.clone())
        .backend(StorageBackend::Disk {
            root: temp_dir.path().to_path_buf(),
        })
        .build();

    let bus = EventBus::new(64);
    let mut hls_rx = bus.subscribe();
    let mut progress_audio = open_packaged_hls_audio(
        &worker,
        &pools,
        &url,
        store.clone(),
        packaged_switch_abr_mode(),
        Some(bus.clone()),
        false,
    )
    .await;
    let abr = progress_audio
        .abr_handle()
        .expect("packaged HLS audio must expose AbrHandle");
    let mut progress_rx = progress_audio.event_bus().subscribe();
    let mut progress_probe = PlaybackProgressProbe::default();
    let mut switch_count = 0usize;
    let mut switch_seen = false;
    let mut total_samples = 0u64;
    let mut post_switch_samples = 0u64;
    let mut consumed = Duration::ZERO;
    let mut reevaluated = false;
    let mut buf = vec![0.0f32; 4096];
    total_samples += read_audio_some(&mut progress_audio, "packaged_abr_warmup").await as u64;
    progress_probe.drain(&mut progress_rx);
    let deadline = Instant::now() + Duration::from_secs(8);

    while Instant::now() < deadline {
        let _ = progress_audio.preload();
        let read: usize = match progress_audio.read(&mut buf) {
            Ok(ReadOutcome::Frames {
                count, position, ..
            }) => {
                if !switch_seen {
                    time::sleep(position.saturating_sub(consumed)).await;
                }
                consumed = position;
                if !reevaluated && consumed >= Duration::from_secs(2) {
                    abr.reevaluate();
                    reevaluated = true;
                }
                count.get()
            }
            Ok(ReadOutcome::Pending { .. }) => 0,
            Ok(ReadOutcome::Eof { .. }) => {
                progress_probe.drain(&mut progress_rx);
                break;
            }
            Err(e) => panic!("decode error: {e}"),
        };
        progress_probe.drain(&mut progress_rx);
        loop {
            match hls_rx.try_recv().map(|env| env.event) {
                Ok(TestEvent::Abr(AbrEvent::VariantApplied { .. })) => {
                    switch_count += 1;
                    switch_seen = true;
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }

        if read == 0 {
            spawn_blocking(|| paced_backoff(Duration::from_millis(10)))
                .await
                .expect("packaged ABR pending pace");
            progress_probe.observe_idle();
            continue;
        }

        total_samples += read as u64;
        if switch_seen {
            post_switch_samples += read as u64;
        }
        if switch_seen && progress_probe.progress_events >= 8 && post_switch_samples >= 16_384 {
            break;
        }
    }
    progress_probe.drain(&mut progress_rx);
    progress_probe.observe_idle();

    assert!(
        switch_count > 0,
        "packaged ABR fixture must switch variants"
    );
    assert!(
        total_samples > 0,
        "packaged ABR fixture must produce decoded output"
    );
    assert!(
        post_switch_samples > 0,
        "packaged ABR fixture produced no decoded output after the switch"
    );
    assert!(
        progress_probe.progress_events >= 4,
        "expected PlaybackProgress events during packaged ABR playback, got {}",
        progress_probe.progress_events
    );
    assert_eq!(
        progress_probe.regressions, 0,
        "PlaybackProgress moved backward during packaged ABR playback"
    );
    assert!(
        progress_probe.max_gap_between_events < Duration::from_millis(1_500),
        "PlaybackProgress stalled for {:?} during packaged ABR playback",
        progress_probe.max_gap_between_events
    );

    // Prime the shared store with EVERY segment of BOTH variants FIRST, on the
    // blocking pool, so the offline render below decodes from cache: the V0
    // segment delay rule would otherwise park the seam read past the render's
    // `hang_timeout_secs`. Forcing each variant with a manual mode guarantees
    // full coverage regardless of which path the offline render's `auto(0)` ABR
    // then takes.
    for variant in [0usize, 1usize] {
        let warm_config = AudioConfig::<Hls<TestPools>>::for_stream(
            HlsConfig::for_url(url.clone())
                .store(store.clone())
                .pools(pools.clone())
                .initial_abr_mode(AbrMode::manual(variant))
                .download_batch_size(1)
                .build(),
        )
        .block_on_underrun(true)
        .build();
        let mut warm_audio = worker
            .load(warm_config)
            .await
            .unwrap_or_else(|e| panic!("packaged ABR warm audio (v{variant}) must open: {e}"));
        let warmed = spawn_blocking(move || {
            let _ = warm_audio.preload();
            let mut buf = vec![0f32; 4096];
            let mut total = 0u64;
            loop {
                match warm_audio.read(&mut buf) {
                    Ok(ReadOutcome::Frames { count, .. }) => total += count.get() as u64,
                    Ok(ReadOutcome::Pending { .. }) => {}
                    Ok(ReadOutcome::Eof { .. }) => break,
                    Err(e) => panic!("packaged ABR warm decode error (v{variant}): {e}"),
                }
            }
            total
        })
        .await
        .expect("packaged ABR warm read join");
        assert!(
            warmed > 0,
            "packaged ABR warm pass (v{variant}) must decode the variant into the shared store"
        );
    }

    let decode_audio = open_packaged_hls_audio(
        &worker,
        &pools,
        &url,
        store,
        packaged_switch_abr_mode(),
        None,
        true,
    )
    .await;
    let mut resource = resource_from_reader(decode_audio);
    let _ = time::timeout(Duration::from_secs(5), resource.preload())
        .await
        .expect("packaged ABR preload must complete");
    let mut player = OfflinePlayer::new(
        HostConfig::offline(pools.clone())
            .settings(
                HostSettings::builder()
                    .sample_rate(
                        NonZeroU32::new(CONTINUITY_SAMPLE_RATE).expect("sample rate is non-zero"),
                    )
                    .build(),
            )
            .build(),
    )
    .await;
    player.load_and_fadein(resource).await;
    render_until_audible(
        &mut player,
        "packaged abr warmup",
        CONTINUITY_BLOCK_FRAMES,
        CONTINUITY_SAMPLE_RATE,
    )
    .await;
    let seam = render_offline_window(
        &mut player,
        220,
        "packaged abr seam window",
        CONTINUITY_BLOCK_FRAMES,
        CONTINUITY_SAMPLE_RATE,
    )
    .await;
    assert_eq!(
        seam.max_silence_run, 0,
        "packaged ABR switch produced {} silent blocks ({seam})",
        seam.max_silence_run
    );
    player.close().await;
}

#[kithara::test(
    tokio,
    native,
    serial,
    timeout(Duration::from_secs(30)),
    hang_timeout_secs(5)
)]
#[cfg_attr(not(target_os = "android"), case::drm_abr_auto_sw(true, true, DecoderBackend::Symphonia, mixed_encrypted().await))]
#[cfg_attr(target_os = "android", case::drm_abr_auto_android(true, true, DecoderBackend::default(), mixed_encrypted().await))]
#[cfg_attr(
    any(target_os = "macos", target_os = "ios"),
    case::drm_abr_auto_hw(true, true, DecoderBackend::Apple, mixed_encrypted().await)
)]
#[cfg_attr(not(target_os = "android"), case::hls_abr_auto_sw(false, true, DecoderBackend::Symphonia, mixed_plain().await))]
#[cfg_attr(target_os = "android", case::hls_abr_auto_android(false, true, DecoderBackend::default(), mixed_plain().await))]
#[cfg_attr(
    any(target_os = "macos", target_os = "ios"),
    case::hls_abr_auto_hw(false, true, DecoderBackend::Apple, mixed_plain().await)
)]
#[cfg_attr(not(target_os = "android"), case::drm_manual_v0_sw(true, false, DecoderBackend::Symphonia, mixed_encrypted().await))]
#[cfg_attr(target_os = "android", case::drm_manual_v0_android(true, false, DecoderBackend::default(), mixed_encrypted().await))]
#[cfg_attr(
    any(target_os = "macos", target_os = "ios"),
    case::drm_manual_v0_hw(true, false, DecoderBackend::Apple, mixed_encrypted().await)
)]
#[cfg_attr(not(target_os = "android"), case::hls_manual_v0_sw(false, false, DecoderBackend::Symphonia, mixed_plain().await))]
#[cfg_attr(target_os = "android", case::hls_manual_v0_android(false, false, DecoderBackend::default(), mixed_plain().await))]
#[cfg_attr(
    any(target_os = "macos", target_os = "ios"),
    case::hls_manual_v0_hw(false, false, DecoderBackend::Apple, mixed_plain().await)
)]
async fn stream_continues_after_seek(
    temp_dir: TestTempDir,
    #[case] encrypted: bool,
    #[case] abr_auto: bool,
    #[case] backend: DecoderBackend,
    #[case] prepared: (TestServerHelper, Url),
) {
    seek::stream_continues_after_seek(temp_dir, encrypted, abr_auto, backend, prepared).await;
}

#[kithara::test(
    tokio,
    native,
    serial,
    timeout(Duration::from_secs(20)),
    hang_timeout_secs(3)
)]
async fn fixed_variant_on_production_ladder_plays_without_hang(
    temp_dir: TestTempDir,
    #[future(awt)] mixed_plain: (TestServerHelper, Url),
) {
    seek::fixed_variant_on_production_ladder_plays_without_hang(temp_dir, mixed_plain).await;
}

#[kithara::test(
    tokio,
    native,
    serial,
    timeout(Duration::from_secs(30)),
    hang_timeout_secs(5),
    tracing("kithara_audio=warn,kithara_hls=warn,symphonia_format_isomp4=warn")
)]
#[case::drm(true, mixed_encrypted().await)]
#[case::hls(false, mixed_plain().await)]
async fn seek_after_eof_mmap_produces_samples(
    temp_dir: TestTempDir,
    #[case] encrypted: bool,
    #[case] prepared: (TestServerHelper, Url),
) {
    seek::seek_after_eof_mmap_produces_samples(temp_dir, encrypted, prepared).await;
}

#[kithara::test(
    tokio,
    native,
    serial,
    timeout(Duration::from_secs(30)),
    hang_timeout_secs(5)
)]
async fn mp3_stream_continues_after_seek(
    temp_dir: TestTempDir,
    #[future(awt)] served_mp3: (TestServerHelper, Url),
) {
    seek::mp3_stream_continues_after_seek(temp_dir, served_mp3).await;
}

/// ABR is locked while a seek is pending, so the seek lands on the variant that
/// played before it even after the estimate fell below that variant. The lock
/// ends with the seek and a switch splices at the decode head, so the first
/// tick after the landing may already move the rest of the landing segment.
///
/// The assertion is on the decision ABR makes, not on how many segment
/// boundaries play before it takes effect. Segments requested while the lock
/// was held still arrive, and how many of them there are depends on how long
/// the seek stayed pending — not on ABR.
#[kithara::test(
    tokio,
    native,
    serial,
    timeout(Duration::from_secs(20)),
    hang_timeout_secs(3),
    tracing("kithara_audio=info,kithara_hls=info")
)]
async fn abr_frozen_during_seek_resumes_after(temp_dir: TestTempDir) {
    use kithara::{audio::AudioRead, signal::AudioChunk};

    const INITIAL_VARIANT: usize = 0;
    const TOP_VARIANT: usize = 3;
    const PREFETCH_SEGMENTS: usize = 3;
    const SEEK_SEGMENT: usize = 25;
    const SLOW_FROM_SEGMENT: usize = SEEK_SEGMENT + 1;
    const SEEK_TARGET: Duration = Duration::from_secs(153);
    const SOFT_TIMEOUT: Duration = Duration::from_secs(10);
    const EVENT_BUS_CAPACITY: usize = 4096;

    let server = TestServerHelper::new().await;
    let pools = pools();
    let cancel = CancelToken::never();
    let net = HttpClient::new(NetOptions::default(), pools.clone(), cancel.child());
    let plain = server
        .create_hls(mixed_codec_ladder())
        .await
        .expect("create the ladder");
    let master = net
        .get_bytes(plain.master_url(), None)
        .await
        .expect("fetch the master playlist");
    let top_bandwidth: u64 = String::from_utf8_lossy(&master)
        .split("BANDWIDTH=")
        .skip(1)
        .filter_map(|rest| {
            rest.split(|c: char| !c.is_ascii_digit())
                .next()?
                .parse()
                .ok()
        })
        .max()
        .expect("the master playlist advertises bandwidths");
    let slow_segment_bytes = net
        .get_bytes(plain.segment_url(TOP_VARIANT, SLOW_FROM_SEGMENT), None)
        .await
        .expect("fetch a top-variant segment")
        .len() as u64;
    // Each slow segment then measures half the top bandwidth.
    let slow_segment_delay_ms = slow_segment_bytes * 8 * 2 * 1000 / top_bandwidth;
    let created = server
        .create_hls(mixed_codec_ladder().delay_rules(vec![DelayRule {
            variant: Some(TOP_VARIANT),
            segment_gte: Some(SLOW_FROM_SEGMENT),
            delay_ms: slow_segment_delay_ms,
            ..Default::default()
        }]))
        .await
        .expect("create the ladder with slow top segments after the seek target");
    // Holds the seek target open, so the seek stays pending while the slow
    // segments after it lower the estimate.
    let seek_segment = server.register_segment_gate(created.token(), TOP_VARIANT, SEEK_SEGMENT);

    let worker = PlayWorker::new(PlayWorkerConfig::builder(pools.clone()).build());
    let downloader = Downloader::new(
        DownloaderConfig::for_client(HttpClient::new(
            NetOptions::default(),
            pools.clone(),
            cancel.child(),
        ))
        .abr_settings(abr_switch_trigger())
        .soft_timeout(SOFT_TIMEOUT)
        .cancel(cancel.child())
        .build(),
    );
    let bus = EventBus::new(EVENT_BUS_CAPACITY);
    let mut events = bus.subscribe();
    let hls_config = HlsConfig::for_url(created.master_url())
        .download_batch_size(PREFETCH_SEGMENTS)
        .store(
            AssetStore::builder(pools.clone())
                .backend(StorageBackend::Disk {
                    root: temp_dir.path().to_path_buf(),
                })
                .build(),
        )
        .pools(pools.clone())
        .initial_abr_mode(auto(INITIAL_VARIANT))
        .cancel(cancel)
        .downloader(downloader)
        .events(bus)
        .build();

    let mut audio = worker
        .load(AudioConfig::<Hls<TestPools>>::for_stream(hls_config).build())
        .await
        .expect("audio creation");
    let _ = audio.preload();

    async fn next_chunk(
        audio: &mut RegisteredAudio<Stream<Hls<TestPools>>, TestPools>,
    ) -> Option<AudioChunk> {
        loop {
            let _ = audio.preload();
            match AudioRead::next_chunk(audio) {
                Ok(ChunkOutcome::Chunk(chunk)) => return Some(chunk),
                Ok(ChunkOutcome::Eof { .. }) => return None,
                Ok(ChunkOutcome::Pending { .. }) => {}
                Err(e) => panic!("decode error in next_chunk: {e}"),
            }
            // Yield so the worker runs and (under flash) the virtual clock
            // advances; wait on the *fact* of a chunk / EOF, never a
            // wall-clock window. A genuine stall trips `KITHARA_HANG_TIMEOUT_SECS`.
            time::sleep(Duration::from_millis(2)).await;
        }
    }

    while next_chunk(&mut audio)
        .await
        .expect("ABR must reach the top variant before the track ends")
        .meta
        .variant_index
        != Some(TOP_VARIANT)
    {}

    let _ = lowest_pending_estimate(&mut events);
    audio.seek(SEEK_TARGET).expect("seek must not fail");
    let pressure_deadline = Instant::now() + Duration::from_millis(slow_segment_delay_ms * 4);
    let mut lowest_while_pending: Option<u64> = None;
    while seek_segment.requested() == 0
        || lowest_while_pending.is_none_or(|bps| bps >= top_bandwidth)
    {
        assert!(
            Instant::now() <= pressure_deadline,
            "the estimate did not fall below the top bandwidth {top_bandwidth} while the seek \
             was pending: lowest {lowest_while_pending:?}, seek target requested {} times",
            seek_segment.requested()
        );
        let _ = audio.preload();
        if let Some(bps) = lowest_pending_estimate(&mut events) {
            lowest_while_pending = Some(lowest_while_pending.map_or(bps, |low| low.min(bps)));
        }
        time::sleep(Duration::from_millis(2)).await;
    }
    seek_segment.release();

    let landing = next_chunk(&mut audio)
        .await
        .expect("playback must resume after the seek");
    assert_eq!(
        landing.meta.segment_index,
        Some(u32::try_from(SEEK_SEGMENT).expect("the seek segment fits u32")),
        "the seek must land on its target segment"
    );
    assert_eq!(
        landing.meta.variant_index,
        Some(TOP_VARIANT),
        "the seek must land on the variant that played before it, although the \
         estimate fell below it while the seek was pending"
    );
    let mut decisions = Vec::new();
    loop {
        let chunk = next_chunk(&mut audio)
            .await
            .expect("ABR must move off the top variant before the track ends");
        let variant = chunk.meta.variant_index.expect("HLS chunk has a variant");
        drain_abr_decisions(&mut events, &mut decisions);
        if variant != TOP_VARIANT {
            assert!(variant < TOP_VARIANT, "ABR must switch down");
            break;
        }
    }
    let unlocked = decisions
        .iter()
        .rposition(|event| matches!(event, AbrEvent::Unlocked))
        .expect("the seek lock must end once the landing output is committed");
    let decided = decisions[unlocked + 1..]
        .iter()
        .find(|event| {
            matches!(
                event,
                AbrEvent::VariantApplied { .. } | AbrEvent::DecisionSkipped { .. }
            )
        })
        .expect("ABR must decide once the seek lock ended");
    assert!(
        matches!(
            decided,
            AbrEvent::VariantApplied { from, to, .. }
                if from.get() == TOP_VARIANT && to.get() < TOP_VARIANT
        ),
        "the first ABR decision after the seek lock ended must leave the top variant, got {decided:?}"
    );
}

#[kithara::test(
    tokio,
    native,
    serial,
    timeout(Duration::from_secs(60)),
    hang_timeout_secs(5)
)]
async fn manual_cross_codec_switch_sustains_post_switch_playback(
    temp_dir: TestTempDir,
    #[future(awt)] mixed_plain: (TestServerHelper, Url),
) {
    packaged::manual_cross_codec_switch_sustains_post_switch_playback(temp_dir, mixed_plain).await;
}
