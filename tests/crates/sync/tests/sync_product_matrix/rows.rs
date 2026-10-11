use super::{cases::*, harness::*, imports::*, providers::*};

#[kithara_test_utils::kithara::hang_watchdog(timeout = Duration::from_secs(5))]
async fn run(case: SyncCase, prepared: PreparedSources, start: Start) {
    let provider = prepared.0;
    let expected_frames: usize = (f64::from(case.sample_rate) * 60.0 / case.ride.final_bpm() * 6.0)
        .round()
        .as_();
    let expected_samples = expected_frames * usize::from(CHANNELS);
    let mut tracks = Vec::with_capacity(case.decks);
    let mut request_failures = Vec::new();
    for audible_deck in 0..case.decks {
        hang_tick!();
        let mut harness =
            ProductHarness::new(case, &prepared, start, Audible::Deck(audible_deck)).await;
        harness.run_operations(case).await;
        harness.ride_tempo(case).await;
        let pcm = harness.capture(case).await;
        assert_eq!(
            pcm.len(),
            expected_samples,
            "{} {provider:?}: deck {audible_deck} capture must contain six complete beats",
            case.id,
        );
        tracks.push(pcm);
        request_failures.append(&mut harness.failures);
        drop(harness);
        hang_reset!();
    }
    let track_slices = tracks.iter().map(Vec::as_slice).collect::<Vec<_>>();
    let label = format!("{} {provider:?}", case.id);
    let mut failures = if provider.has_score_markers() {
        marked_synchronization_failures(
            &label,
            &track_slices,
            CHANNELS,
            case.sample_rate,
            case.ride.final_bpm(),
        )
    } else {
        synchronization_failures(
            &label,
            &track_slices,
            CHANNELS,
            case.sample_rate,
            case.ride.final_bpm(),
        )
    };
    failures.extend(request_failures);
    assert!(
        failures.is_empty(),
        "ignored-red product synchronization assertion failed for {} {provider:?}:\n{}",
        case.id,
        failures.join("\n"),
    );
}

#[kithara::test(native, tokio, multi_thread, serial, timeout(Duration::from_secs(60)))]
#[case::mp3(source_mp3_same().await)]
#[case::drm(source_hls_same_drm().await)]
async fn encoded_rhythmic_controls_reach_the_pcm_oracle(#[case] prepared: PreparedSources) {
    let provider = prepared.0;
    let mut harness = ProductHarness::new(ONE_DECK, &prepared, CUE, Audible::Deck(0)).await;
    let pcm = harness.capture(ONE_DECK).await;
    let mut failures = synchronization_failures(
        &format!("encoded rhythmic control {provider:?}"),
        &[pcm.as_slice()],
        CHANNELS,
        ONE_DECK.sample_rate,
        START_BPM,
    );
    failures.append(&mut harness.failures);
    assert!(
        failures.is_empty(),
        "encoded rhythmic control {provider:?} failed:\n{}",
        failures.join("\n"),
    );
    drop(harness);
}

#[kithara::test(native, tokio, multi_thread, serial, timeout(Duration::from_secs(600)))]
#[case::play_sync_seek(PLAY_SYNC_SEEK, source_synthetic().await)]
#[case::play_seek_sync(PLAY_SEEK_SYNC, source_synthetic().await)]
#[case::seek_play_sync(SEEK_PLAY_SYNC, source_synthetic().await)]
#[case::seek_sync_play(SEEK_SYNC_PLAY, source_synthetic().await)]
#[case::sync_play_seek(SYNC_PLAY_SEEK, source_synthetic().await)]
#[case::sync_seek_play(SYNC_SEEK_PLAY, source_synthetic().await)]
#[case::sequential_sync(SEQUENTIAL_SYNC, source_synthetic().await)]
#[case::paused_sync_then_play(PAUSED_SYNC, source_synthetic().await)]
#[case::four_deck_sequential_sync(FOUR_DECK_SYNC, source_synthetic().await)]
#[case::tempo_up_120hz(TEMPO_UP_120, source_synthetic().await)]
#[case::tempo_down_30hz(TEMPO_DOWN_30, source_synthetic().await)]
#[case::ambient_trip_hop(AMBIENT_TRIP_HOP_SYNC, source_ambient_trip_hop_provider().await)]
#[case::downtempo_house(DOWNTEMPO_HOUSE_SYNC, source_downtempo_house_provider().await)]
#[case::techno_breakbeat(TECHNO_BREAKBEAT_SYNC, source_techno_breakbeat_provider().await)]
#[case::cross_style_four_deck(CROSS_STYLE_SYNC, source_cross_style_provider().await)]
async fn wav_product_rows_reach_the_pcm_oracle(
    #[case] case: SyncCase,
    #[case] provider: PreparedSources,
) {
    run(case, provider, CUE).await;
}

#[kithara::test(native, tokio, multi_thread, serial, timeout(Duration::from_secs(600)))]
#[case::hls_same_play_sync_seek(source_hls_same_plain().await, PLAY_SYNC_SEEK)]
#[case::hls_same_play_seek_sync(source_hls_same_plain().await, PLAY_SEEK_SYNC)]
#[case::hls_same_seek_play_sync(source_hls_same_plain().await, SEEK_PLAY_SYNC)]
#[case::hls_same_seek_sync_play(source_hls_same_plain().await, SEEK_SYNC_PLAY)]
#[case::hls_same_sync_play_seek(source_hls_same_plain().await, SYNC_PLAY_SEEK)]
#[case::hls_same_sync_seek_play(source_hls_same_plain().await, SYNC_SEEK_PLAY)]
#[case::hls_same_sequential_sync(source_hls_same_plain().await, SEQUENTIAL_SYNC)]
#[case::hls_same_paused_sync_then_play(source_hls_same_plain().await, PAUSED_SYNC)]
#[case::hls_same_four_deck_sequential_sync(source_hls_same_plain().await, FOUR_DECK_SYNC)]
#[case::hls_same_tempo_up_120hz(source_hls_same_plain().await, TEMPO_UP_120)]
#[case::hls_same_tempo_down_30hz(source_hls_same_plain().await, TEMPO_DOWN_30)]
#[case::drm_same_play_sync_seek(source_hls_same_drm().await, PLAY_SYNC_SEEK)]
#[case::drm_same_play_seek_sync(source_hls_same_drm().await, PLAY_SEEK_SYNC)]
#[case::drm_same_seek_play_sync(source_hls_same_drm().await, SEEK_PLAY_SYNC)]
#[case::drm_same_seek_sync_play(source_hls_same_drm().await, SEEK_SYNC_PLAY)]
#[case::drm_same_sync_play_seek(source_hls_same_drm().await, SYNC_PLAY_SEEK)]
#[case::drm_same_sync_seek_play(source_hls_same_drm().await, SYNC_SEEK_PLAY)]
#[case::drm_same_sequential_sync(source_hls_same_drm().await, SEQUENTIAL_SYNC)]
#[case::drm_same_paused_sync_then_play(source_hls_same_drm().await, PAUSED_SYNC)]
#[case::drm_same_four_deck_sequential_sync(source_hls_same_drm().await, FOUR_DECK_SYNC)]
#[case::drm_same_tempo_up_120hz(source_hls_same_drm().await, TEMPO_UP_120)]
#[case::drm_same_tempo_down_30hz(source_hls_same_drm().await, TEMPO_DOWN_30)]
#[case::mp3_same_play_sync_seek(source_mp3_same().await, PLAY_SYNC_SEEK)]
#[case::mp3_same_play_seek_sync(source_mp3_same().await, PLAY_SEEK_SYNC)]
#[case::mp3_same_seek_play_sync(source_mp3_same().await, SEEK_PLAY_SYNC)]
#[case::mp3_same_seek_sync_play(source_mp3_same().await, SEEK_SYNC_PLAY)]
#[case::mp3_same_sync_play_seek(source_mp3_same().await, SYNC_PLAY_SEEK)]
#[case::mp3_same_sync_seek_play(source_mp3_same().await, SYNC_SEEK_PLAY)]
#[case::mp3_same_sequential_sync(source_mp3_same().await, SEQUENTIAL_SYNC)]
#[case::mp3_same_paused_sync_then_play(source_mp3_same().await, PAUSED_SYNC)]
#[case::mp3_same_four_deck_sequential_sync(source_mp3_same().await, FOUR_DECK_SYNC)]
#[case::mp3_same_tempo_up_120hz(source_mp3_same().await, TEMPO_UP_120)]
#[case::mp3_same_tempo_down_30hz(source_mp3_same().await, TEMPO_DOWN_30)]
#[case::mp3_distinct_play_sync_seek(source_mp3_distinct().await, PLAY_SYNC_SEEK)]
#[case::mp3_distinct_play_seek_sync(source_mp3_distinct().await, PLAY_SEEK_SYNC)]
#[case::mp3_distinct_seek_play_sync(source_mp3_distinct().await, SEEK_PLAY_SYNC)]
#[case::mp3_distinct_seek_sync_play(source_mp3_distinct().await, SEEK_SYNC_PLAY)]
#[case::mp3_distinct_sync_play_seek(source_mp3_distinct().await, SYNC_PLAY_SEEK)]
#[case::mp3_distinct_sync_seek_play(source_mp3_distinct().await, SYNC_SEEK_PLAY)]
#[case::mp3_distinct_sequential_sync(source_mp3_distinct().await, SEQUENTIAL_SYNC)]
#[case::mp3_distinct_paused_sync_then_play(source_mp3_distinct().await, PAUSED_SYNC)]
#[case::mp3_distinct_four_deck_sequential_sync(source_mp3_distinct().await, FOUR_DECK_SYNC)]
#[case::mp3_distinct_tempo_up_120hz(source_mp3_distinct().await, TEMPO_UP_120)]
#[case::mp3_distinct_tempo_down_30hz(source_mp3_distinct().await, TEMPO_DOWN_30)]
#[case::hls_mp3_play_sync_seek(source_hls_mp3_plain().await, PLAY_SYNC_SEEK)]
#[case::hls_mp3_play_seek_sync(source_hls_mp3_plain().await, PLAY_SEEK_SYNC)]
#[case::hls_mp3_seek_play_sync(source_hls_mp3_plain().await, SEEK_PLAY_SYNC)]
#[case::hls_mp3_seek_sync_play(source_hls_mp3_plain().await, SEEK_SYNC_PLAY)]
#[case::hls_mp3_sync_play_seek(source_hls_mp3_plain().await, SYNC_PLAY_SEEK)]
#[case::hls_mp3_sync_seek_play(source_hls_mp3_plain().await, SYNC_SEEK_PLAY)]
#[case::hls_mp3_sequential_sync(source_hls_mp3_plain().await, SEQUENTIAL_SYNC)]
#[case::hls_mp3_paused_sync_then_play(source_hls_mp3_plain().await, PAUSED_SYNC)]
#[case::hls_mp3_four_deck_sequential_sync(source_hls_mp3_plain().await, FOUR_DECK_SYNC)]
#[case::hls_mp3_tempo_up_120hz(source_hls_mp3_plain().await, TEMPO_UP_120)]
#[case::hls_mp3_tempo_down_30hz(source_hls_mp3_plain().await, TEMPO_DOWN_30)]
#[case::drm_mp3_play_sync_seek(source_hls_mp3_drm().await, PLAY_SYNC_SEEK)]
#[case::drm_mp3_play_seek_sync(source_hls_mp3_drm().await, PLAY_SEEK_SYNC)]
#[case::drm_mp3_seek_play_sync(source_hls_mp3_drm().await, SEEK_PLAY_SYNC)]
#[case::drm_mp3_seek_sync_play(source_hls_mp3_drm().await, SEEK_SYNC_PLAY)]
#[case::drm_mp3_sync_play_seek(source_hls_mp3_drm().await, SYNC_PLAY_SEEK)]
#[case::drm_mp3_sync_seek_play(source_hls_mp3_drm().await, SYNC_SEEK_PLAY)]
#[case::drm_mp3_sequential_sync(source_hls_mp3_drm().await, SEQUENTIAL_SYNC)]
#[case::drm_mp3_paused_sync_then_play(source_hls_mp3_drm().await, PAUSED_SYNC)]
#[case::drm_mp3_four_deck_sequential_sync(source_hls_mp3_drm().await, FOUR_DECK_SYNC)]
#[case::drm_mp3_tempo_up_120hz(source_hls_mp3_drm().await, TEMPO_UP_120)]
#[case::drm_mp3_tempo_down_30hz(source_hls_mp3_drm().await, TEMPO_DOWN_30)]
async fn real_media_product_rows_reach_the_pcm_oracle(
    #[case] provider: PreparedSources,
    #[case] case: SyncCase,
) {
    run(case, provider, CUE).await;
}

#[kithara::test(native, tokio, multi_thread, serial, timeout(Duration::from_secs(600)))]
#[case::tunnel_play_sync_seek(tunnel_sources().await, TUNNEL_CUE, PLAY_SYNC_SEEK.gridded())]
#[case::tunnel_play_seek_sync(tunnel_sources().await, TUNNEL_CUE, PLAY_SEEK_SYNC.gridded())]
#[case::tunnel_seek_play_sync(tunnel_sources().await, TUNNEL_CUE, SEEK_PLAY_SYNC.gridded())]
#[case::tunnel_seek_sync_play(tunnel_sources().await, TUNNEL_CUE, SEEK_SYNC_PLAY.gridded())]
#[case::tunnel_sync_play_seek(tunnel_sources().await, TUNNEL_CUE, SYNC_PLAY_SEEK.gridded())]
#[case::tunnel_sync_seek_play(tunnel_sources().await, TUNNEL_CUE, SYNC_SEEK_PLAY.gridded())]
#[case::tunnel_sequential_sync(tunnel_sources().await, TUNNEL_CUE, SEQUENTIAL_SYNC.gridded())]
#[case::tunnel_paused_sync_then_play(tunnel_sources().await, TUNNEL_CUE, PAUSED_SYNC.gridded())]
#[case::tunnel_four_deck_sequential_sync(tunnel_sources().await, TUNNEL_CUE, FOUR_DECK_SYNC.gridded())]
#[case::tunnel_tempo_up_120hz(tunnel_sources().await, TUNNEL_CUE, TEMPO_UP_120.gridded())]
#[case::tunnel_tempo_down_30hz(tunnel_sources().await, TUNNEL_CUE, TEMPO_DOWN_30.gridded())]
#[case::newtechno_play_sync_seek(newtechno_sources().await, NEWTECHNO_PHRASE, PLAY_SYNC_SEEK.gridded())]
#[case::newtechno_play_seek_sync(newtechno_sources().await, NEWTECHNO_PHRASE, PLAY_SEEK_SYNC.gridded())]
#[case::newtechno_seek_play_sync(newtechno_sources().await, NEWTECHNO_PHRASE, SEEK_PLAY_SYNC.gridded())]
#[case::newtechno_seek_sync_play(newtechno_sources().await, NEWTECHNO_PHRASE, SEEK_SYNC_PLAY.gridded())]
#[case::newtechno_sync_play_seek(newtechno_sources().await, NEWTECHNO_PHRASE, SYNC_PLAY_SEEK.gridded())]
#[case::newtechno_sync_seek_play(newtechno_sources().await, NEWTECHNO_PHRASE, SYNC_SEEK_PLAY.gridded())]
#[case::newtechno_sequential_sync(newtechno_sources().await, NEWTECHNO_PHRASE, SEQUENTIAL_SYNC.gridded())]
#[case::newtechno_paused_sync_then_play(newtechno_sources().await, NEWTECHNO_PHRASE, PAUSED_SYNC.gridded())]
#[case::newtechno_four_deck_sequential_sync(newtechno_sources().await, NEWTECHNO_PHRASE, FOUR_DECK_SYNC.gridded())]
#[case::newtechno_tempo_up_120hz(newtechno_sources().await, NEWTECHNO_PHRASE, TEMPO_UP_120.gridded())]
#[case::newtechno_tempo_down_30hz(newtechno_sources().await, NEWTECHNO_PHRASE, TEMPO_DOWN_30.gridded())]
async fn real_track_product_rows_reach_the_pcm_oracle(
    #[case] provider: PreparedSources,
    #[case] start: Start,
    #[case] case: SyncCase,
) {
    run(case, provider, start).await;
}

#[kithara::fixture]
pub(crate) async fn synthetic_sources() -> PreparedSources {
    prepared_sources(Provider::Synthetic).await
}
#[kithara::fixture]
pub(crate) async fn sweep_sources() -> PreparedSources {
    prepared_sources(Provider::Sweep).await
}
#[kithara::fixture]
pub(crate) async fn mixed_sources() -> PreparedSources {
    prepared_sources(Provider::HlsMp3(HlsProtection::Plain)).await
}

#[kithara::fixture]
async fn source_mp3_same() -> PreparedSources {
    prepared_sources(Provider::Mp3Same).await
}

#[kithara::fixture]
async fn source_hls_same_drm() -> PreparedSources {
    prepared_sources(Provider::HlsSame(HlsProtection::Drm)).await
}

#[kithara::fixture]
async fn source_synthetic() -> PreparedSources {
    prepared_sources(Provider::Synthetic).await
}

#[kithara::fixture]
async fn source_ambient_trip_hop_provider() -> PreparedSources {
    prepared_sources(AMBIENT_TRIP_HOP_PROVIDER).await
}

#[kithara::fixture]
async fn source_downtempo_house_provider() -> PreparedSources {
    prepared_sources(DOWNTEMPO_HOUSE_PROVIDER).await
}

#[kithara::fixture]
async fn source_techno_breakbeat_provider() -> PreparedSources {
    prepared_sources(TECHNO_BREAKBEAT_PROVIDER).await
}

#[kithara::fixture]
async fn source_cross_style_provider() -> PreparedSources {
    prepared_sources(CROSS_STYLE_PROVIDER).await
}

#[kithara::fixture]
async fn source_hls_same_plain() -> PreparedSources {
    prepared_sources(Provider::HlsSame(HlsProtection::Plain)).await
}

#[kithara::fixture]
async fn source_mp3_distinct() -> PreparedSources {
    prepared_sources(Provider::Mp3Distinct).await
}

#[kithara::fixture]
async fn source_hls_mp3_plain() -> PreparedSources {
    prepared_sources(Provider::HlsMp3(HlsProtection::Plain)).await
}

#[kithara::fixture]
async fn source_hls_mp3_drm() -> PreparedSources {
    prepared_sources(Provider::HlsMp3(HlsProtection::Drm)).await
}

#[kithara::fixture]
pub(crate) async fn tunnel_sources() -> PreparedSources {
    prepared_sources(Provider::Library(TUNNEL)).await
}

#[kithara::fixture]
pub(crate) async fn newtechno_sources() -> PreparedSources {
    prepared_sources(Provider::Library(NEWTECHNO)).await
}
