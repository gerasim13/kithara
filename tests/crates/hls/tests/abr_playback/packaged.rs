use kithara::{
    abr::{AbrEvent, AbrReason},
    assets::{AssetStore, StorageBackend},
    audio::{AudioConfig, AudioControl, AudioRead, AudioSession, ReadOutcome},
    download::{Downloader, DownloaderConfig},
    events::{EventBus, EventReceiver},
    hls::{AbrMode, Hls, HlsConfig},
    net::{HttpClient, NetOptions},
    platform::{
        CancelToken, time,
        time::{Duration, Instant},
        tokio::task::spawn_blocking,
    },
    play::{PlayWorker, PlayWorkerConfig, RegisteredAudio},
    stream::{AudioCodec, Stream},
};
use kithara_integration_tests::{
    HlsFixtureBuilder, TestServerHelper, abr_fast, auto,
    bufpool_ext::{Pools, TestPools, pools},
    event::TestEvent,
    fixture_protocol::{DelayRule, PcmPattern},
    output_continuity::CONTINUITY_SAMPLE_RATE,
};
use kithara_test_utils::{TestTempDir, pace};
use tracing::info;
use url::Url;

pub(crate) fn packaged_switch_abr_mode() -> AbrMode {
    auto(0)
}

fn packaged_identical_content_abr_builder(codec: AudioCodec) -> HlsFixtureBuilder {
    let builder = HlsFixtureBuilder::new()
        .variant_count(2)
        .segments_per_variant(6)
        .segment_duration_secs(2.0)
        .variant_bandwidths(vec![10_000, 50_000])
        .delay_rules(vec![DelayRule {
            variant: Some(0),
            segment_eq: Some(2),
            delay_ms: 500,
            ..Default::default()
        }]);
    match codec {
        AudioCodec::AacLc => builder.packaged_audio_per_variant_pcm_aac_lc(
            CONTINUITY_SAMPLE_RATE,
            2,
            vec![PcmPattern::Ascending, PcmPattern::Ascending],
        ),
        AudioCodec::Flac => builder.packaged_audio_per_variant_pcm_flac(
            CONTINUITY_SAMPLE_RATE,
            2,
            vec![PcmPattern::Ascending, PcmPattern::Ascending],
        ),
        other => panic!("unsupported packaged ABR codec: {other:?}"),
    }
}

#[kithara::fixture]
pub(crate) async fn create_packaged_abr_fixture() -> (TestServerHelper, Url) {
    let server = TestServerHelper::new().await;
    let created = server
        .create_hls(packaged_identical_content_abr_builder(AudioCodec::AacLc))
        .await
        .unwrap_or_else(|error| panic!("create packaged ABR fixture: {error}"));
    (server, created.master_url())
}

/// Open the packaged ABR fixture as decoded audio.
///
/// `block_on_underrun` decides what an empty producer ring returns. A caller
/// that drives the read loop itself and handles [`ReadOutcome::Pending`] wants
/// `false`; a caller that reads the rendered PCM back wants `true`, because a
/// zero-filled range is indistinguishable from rendered silence.
pub(crate) async fn open_packaged_hls_audio(
    worker: &PlayWorker<TestPools>,
    pools: &Pools,
    url: &Url,
    store: AssetStore<TestPools>,
    abr: AbrMode,
    bus: Option<EventBus>,
    block_on_underrun: bool,
) -> RegisteredAudio<Stream<Hls<TestPools>>, TestPools> {
    let cancel = CancelToken::never();
    let downloader = Downloader::new(
        DownloaderConfig::for_client(HttpClient::new(
            NetOptions::default(),
            pools.clone(),
            cancel.child(),
        ))
        .abr_settings(abr_fast())
        .cancel(cancel.child())
        .build(),
    );
    let hls_config = HlsConfig::for_url(url.clone())
        .store(store)
        .pools(pools.clone())
        .initial_abr_mode(abr)
        .download_batch_size(1)
        .cancel(cancel)
        .downloader(downloader)
        .maybe_events(bus.clone())
        .build();

    let config = AudioConfig::<Hls<TestPools>>::for_stream(hls_config)
        .maybe_events(bus)
        .block_on_underrun(block_on_underrun)
        .build();

    let mut audio = worker
        .load(config)
        .await
        .unwrap_or_else(|err| panic!("packaged ABR audio should open for {url}: {err}"));
    let _ = audio.preload();
    audio
}

pub(crate) async fn read_audio_some(
    audio: &mut RegisteredAudio<Stream<Hls<TestPools>>, TestPools>,
    stage: &str,
) -> usize {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut buf = [0.0f32; 4096];

    loop {
        let _ = audio.preload();
        match audio.read(&mut buf) {
            Ok(ReadOutcome::Frames { count, .. }) => return count.get(),
            Ok(ReadOutcome::Pending { .. }) => {}
            Ok(ReadOutcome::Eof { .. }) => {
                panic!("unexpected EOF while waiting for packaged ABR audio at stage={stage}");
            }
            Err(e) => panic!("decode error at stage={stage}: {e}"),
        }
        assert!(
            Instant::now() <= deadline,
            "timed out waiting for packaged ABR audio at stage={stage}"
        );
        time::sleep(Duration::from_millis(10)).await;
    }
}

/// Real fMP4/AAC HLS stream with ABR auto-switch must play without hanging.
///
/// This is the exact scenario from the production crash:
/// `kithara-app` plays one MP3 plus the plain and encrypted HLS masters.
/// ABR switches variant on HLS track → worker hangs → all tracks die.
pub(crate) async fn abr_switch_on_production_ladder_does_not_hang(
    temp_dir: TestTempDir,
    mixed_plain: (TestServerHelper, Url),
) {
    let (_server, url) = mixed_plain;

    let cancel = CancelToken::never();
    let pools = pools();
    let worker = PlayWorker::new(
        PlayWorkerConfig::builder(pools.clone())
            .cancel(cancel.clone())
            .build(),
    );
    let hls_config = HlsConfig::for_url(url)
        .store(
            AssetStore::builder(pools.clone())
                .backend(StorageBackend::Disk {
                    root: temp_dir.path().to_path_buf(),
                })
                .build(),
        )
        .pools(pools.clone())
        .cancel(cancel)
        .initial_abr_mode(auto(0))
        .build();

    let config = AudioConfig::<Hls<TestPools>>::for_stream(hls_config)
        .block_on_underrun(true)
        .build();
    let mut audio = worker.load(config).await.expect("create audio");
    spawn_blocking(move || {
        let _ = audio.preload();
        let mut buf = vec![0f32; 4096];
        let mut total_samples = 0u64;

        let mut saw_eof = false;
        loop {
            match audio.read(&mut buf) {
                Ok(ReadOutcome::Pending { .. }) => {
                    pace(Duration::from_millis(10));
                }
                Ok(ReadOutcome::Frames { count, .. }) => {
                    total_samples += count.get() as u64;
                    if total_samples >= 100_000 {
                        break;
                    }
                }
                Ok(ReadOutcome::Eof { .. }) => {
                    saw_eof = true;
                    break;
                }
                Err(e) => panic!("decode error: {e}"),
            }
        }

        let _ = saw_eof;
        info!(total_samples, "playback completed without hang");
        assert!(
            total_samples > 1000,
            "expected sustained playback, got only {total_samples} samples"
        );
    })
    .await
    .expect("read phase join");
}

#[derive(Debug)]
struct CrossCodecReadStats {
    post_samples: u64,
    applied_transitions: Vec<(usize, AbrReason)>,
    max_stall_ms: u128,
    saw_eof: bool,
}

#[kithara::flash(true)]
fn read_manual_cross_codec_phase(
    audio: &mut RegisteredAudio<Stream<Hls<TestPools>>, TestPools>,
    hls_rx: &mut EventReceiver<TestEvent>,
    post_target: u64,
) -> CrossCodecReadStats {
    let mut buf = vec![0f32; 4096];
    let mut stats = CrossCodecReadStats {
        post_samples: 0,
        applied_transitions: Vec::new(),
        max_stall_ms: 0,
        saw_eof: false,
    };
    let mut transition_samples = 0u64;
    let mut consumed = Duration::ZERO;
    let mut manual_applied = false;
    let mut last_progress = Instant::now();

    while !manual_applied || stats.post_samples < post_target {
        while let Ok(event) = hls_rx.try_recv().map(|envelope| envelope.event) {
            if let TestEvent::Abr(AbrEvent::VariantApplied { to, reason, .. }) = event {
                let transition = (to.get(), reason);
                stats.applied_transitions.push(transition);
                if !manual_applied && transition == (3, AbrReason::ManualOverride) {
                    manual_applied = true;
                    stats.post_samples = 0;
                    stats.max_stall_ms = 0;
                    last_progress = Instant::now();
                }
            }
        }

        match audio.read(&mut buf) {
            Ok(ReadOutcome::Frames {
                count, position, ..
            }) => {
                let count = u64::try_from(count.get())
                    .unwrap_or_else(|error| panic!("read count does not fit u64: {error}"));
                if manual_applied {
                    stats.post_samples += count;
                    last_progress = Instant::now();
                } else {
                    transition_samples += count;
                    if transition_samples >= 17_640 {
                        pace(position.saturating_sub(consumed));
                    }
                    consumed = position;
                }
            }
            Ok(ReadOutcome::Pending { .. }) => {}
            Ok(ReadOutcome::Eof { .. }) if !manual_applied => {
                panic!(
                    "EOF before Manual(3) produced VariantApplied{{to:3, \
                     reason:ManualOverride}}; observed {:?}",
                    stats.applied_transitions
                );
            }
            Ok(ReadOutcome::Eof { .. }) => {
                stats.saw_eof = true;
                break;
            }
            Err(error) => panic!("decode error post-switch: {error}"),
        }

        if manual_applied {
            let stalled = Instant::now().duration_since(last_progress).as_millis();
            if stalled > stats.max_stall_ms {
                stats.max_stall_ms = stalled;
            }
        }
    }

    stats
}

/// Phase P.0 regression: production bug repro from app.log (2026-05-15).
///
/// Scenario in prod (app.log:103-104):
/// `commit_variant_switch from=0 to=3 cross_codec=true reason=UpSwitch`
/// fires on the FIRST boundary crossing. Decoder produces some samples
/// then stalls; `decode_next_chunk` hangs ~10s later. User experience:
/// "audio disappeared, the slider kept moving, then the app crashed".
///
/// `abr_switch_on_production_ladder_does_not_hang` and `runtime_cross_codec_manual_switch_no_hang`
/// already cover the cross-codec path but their assertions accept
/// `total_samples > 0` / `> 1000` — a single post-init buffer satisfies
/// them, masking a stall. Sustained playback requires a much higher bar.
///
/// Deterministic structure: open Auto(0)=AAC, read >=200 ms of audio to
/// confirm AAC decoder warmed, then trigger Manual(3)=FLAC. The Manual
/// path runs through the same `commit_variant_switch` cross-codec branch
/// as Auto — deterministic timing avoids the Auto-only flakiness where
/// the bandwidth estimator may or may not commit the switch within the
/// test window.
///
/// The mixed-codec ladder carries variants 0-2 AAC (mp4a.40.2) and
/// variant 3 FLAC — 37 segments × 6 s each, so about 220 s of audio.
/// Reading 15 s post-switch must produce at least
/// `15 × 44_100 × 2 × 0.5 = 661_500` samples (50 % of nominal rate).
pub(crate) async fn manual_cross_codec_switch_sustains_post_switch_playback(
    temp_dir: TestTempDir,
    mixed_plain: (TestServerHelper, Url),
) {
    let (_server, url) = mixed_plain;

    let cancel = CancelToken::never();
    let bus = EventBus::new(8192);
    let pools = pools();
    let worker = PlayWorker::new(
        PlayWorkerConfig::builder(pools.clone())
            .cancel(cancel.clone())
            .build(),
    );

    let hls_config = HlsConfig::for_url(url)
        .store(
            AssetStore::builder(pools.clone())
                .backend(StorageBackend::Disk {
                    root: temp_dir.path().to_path_buf(),
                })
                .build(),
        )
        .pools(pools.clone())
        .cancel(cancel)
        .events(bus.clone())
        .initial_abr_mode(auto(0))
        .build();

    // `block_on_underrun(true)` makes every `read()` park on the worker via
    // `recv_outcome_blocking` (`#[kithara::flash(true)]` → `park_timeout` on the
    // virtual clock) rather than zero-filling on underrun. The park is what
    // advances virtual time, so the worker keeps delivering real decoded chunks
    // and the post-switch window accumulates true FLAC frames. A genuine
    // post-switch decoder stall then parks forever, the virtual clock cannot
    // pass the hang budget, and the `KITHARA_HANG_TIMEOUT_SECS=5` watchdog fires
    // — the flash-correct enforcement of the "no >5 s stall" contract.
    let config = AudioConfig::<Hls<TestPools>>::for_stream(hls_config)
        .events(bus.clone())
        .block_on_underrun(true)
        .build();
    let mut audio = worker.load(config).await.expect("create audio");

    // Subscribe before any switch so `VariantApplied{to:3}` cannot be missed.
    // The post-switch read owns this receiver and drains it inline.
    let mut hls_rx = bus.subscribe();

    // Phase 1 — AAC warmup. Read until we have at least 200 ms of audio
    // (44_100 × 2 × 0.2 = 17_640 frames) so AAC decoder is fully primed. The
    // blocking read must run off the test runtime thread: parking it here would
    // starve the HLS drive/fetch tasks that feed the worker.
    let (mut audio, pre_samples) = spawn_blocking(move || {
        let _ = audio.preload();
        let mut buf = vec![0f32; 4096];
        let mut pre = 0u64;
        while pre < 17_640 {
            match audio.read(&mut buf) {
                Ok(ReadOutcome::Frames { count, .. }) => pre += count.get() as u64,
                Ok(ReadOutcome::Pending { .. }) => {}
                Ok(ReadOutcome::Eof { .. }) => break,
                Err(e) => panic!("decode error during AAC warmup: {e}"),
            }
        }
        (audio, pre)
    })
    .await
    .expect("AAC warmup join");
    assert!(
        pre_samples >= 17_640,
        "AAC warmup must produce ≥17_640 frames before the cross-codec flip; \
         got {pre_samples}"
    );

    // Phase 2 — trigger cross-codec Manual switch to FLAC. Runs on the runtime
    // thread between the blocking read phases.
    let handle = audio
        .abr_handle()
        .expect("HLS stream must expose AbrHandle");
    handle
        .set_mode(AbrMode::manual(3))
        .expect("Manual(3) (FLAC) target must be valid");

    // Phase 3 — sustained post-switch playback. Bound by a SAMPLE TARGET, not a
    // wall clock: under flash a parked read collapses real time to ~zero, so a
    // `Instant::now() + 15 s` loop would drain the whole 148 s tail and hit a
    // legitimate end-of-track EOF before any real time elapsed. Reading the
    // ≥660_000-frame target proves sustained FLAC throughput; `saw_eof` records
    // a PREMATURE end (the prod stall surfaces as a false EOS), and the read
    // owns `hls_rx` to capture every `VariantApplied` without broadcast lag.
    //
    // The sample target alone does not bound this window, though: a consumer
    // reading a warm cache produces those frames in less wall-clock time than a
    // single fetch takes, so the whole budget can be spent on the OUTGOING
    // variant and the window closes before the switch it is meant to observe
    // has issued one request. That measures nothing "post-switch". The blocking
    // phase therefore paces the outgoing reader after the same 200 ms sample
    // floor as warmup, then resets its counter only after the exact Manual(3)
    // commit event. The harness timeout, not the sample budget, fails a switch
    // that never comes.
    let post_target: u64 = 660_000;
    let stats =
        spawn_blocking(move || read_manual_cross_codec_phase(&mut audio, &mut hls_rx, post_target))
            .await
            .expect("sustained post-switch read join");

    info!(
        applied_transitions = ?stats.applied_transitions,
        pre_samples,
        post_samples = stats.post_samples,
        max_stall_ms = stats.max_stall_ms,
        saw_eof = stats.saw_eof,
        "Phase P.0: cross-codec switch sustained playback"
    );

    assert!(
        stats
            .applied_transitions
            .contains(&(3, AbrReason::ManualOverride)),
        "Manual(3) must surface VariantApplied{{to:3, reason:ManualOverride}} (FLAC) — \
         applied_transitions={:?}",
        stats.applied_transitions
    );
    assert!(
        !stats.saw_eof,
        "FLAC playback after the cross-codec flip ended prematurely (false EOS) \
         before the ≥660_000-frame target; got {}. \
         Prod symptom: decoder stalls / dies after the switch.",
        stats.post_samples
    );
    assert!(
        stats.post_samples >= 660_000,
        "sustained FLAC playback after cross-codec flip must produce \
         ≥660_000 frames (≈50 % nominal 44.1 kHz × 15 s × 2 ch); \
         got {}. Prod symptom: decoder stalls after the switch.",
        stats.post_samples
    );
    assert!(
        stats.max_stall_ms < 5_000,
        "decoder must not stall longer than 5 s after the cross-codec flip; \
         longest stall window was {} ms",
        stats.max_stall_ms
    );
}
