#![cfg(not(target_arch = "wasm32"))]

use std::path::Path;

use kithara::{
    assets::{AssetStore, StorageBackend},
    events::{PlayerEvent, TrackId},
    platform::time::{self, Duration},
    play::{Resource, ResourceConfig},
    warp::{StretchControls, StretchKind},
};
use kithara_integration_tests::{
    TestTempDir,
    audio_fixture::EmbeddedAudio,
    create_test_wav,
    goertzel::goertzel_magnitude,
    kithara,
    offline::{OfflinePlayerHarness, OfflinePlayerOptions},
    temp_dir,
    wav::prepare_sine_wav,
};

const SAMPLE_RATE: u32 = 44_100;
const CHANNELS: u16 = 2;
const BLOCK_FRAMES: usize = 512;
/// Long enough for the ring to have flushed every block decoded at the
/// previous rate, so the measured window sees only the new one.
const SETTLE_BLOCKS: usize = 400;
const MEASURE_BLOCKS: usize = 400;
const FAST_RATE: f32 = 2.0;
const DRAIN_FIXTURE_SECS: usize = 4;
const DRAIN_BLOCK_BUDGET: usize = 4_000;
/// The accelerated run must reach EOF within this share of the rate-1.0
/// output. Deliberately far above the ~0.53 the pipeline actually delivers:
/// the discrimination that matters is against an unchanged 1.0 ratio.
const DRAIN_SHARE_NUM: usize = 3;
const DRAIN_SHARE_DEN: usize = 4;
const SOURCE_TONE_HZ: f64 = 440.0;
const SLOW_RATE: f32 = 0.5;
const SLOW_TONE_HZ: f64 = 220.0;
const FAST_TONE_HZ: f64 = 880.0;
const TONE_DOMINANCE_RATIO: f64 = 4.0;
const WARMUP_BLOCK_BUDGET: usize = 200;
/// Declared native latency, rounded up to 512 frames, plus one frequency-analysis block.
const SIGNALSMITH_RESPONSE_BLOCK_BUDGET: usize = 7;
const BUNGEE_RESPONSE_BLOCK_BUDGET: usize = 15;
const RESPONSE_OBSERVATION_BLOCK_BUDGET: usize = 400;
const FIXTURE_SECONDS: usize = 12;

async fn file_resource(harness: &OfflinePlayerHarness, path: &Path, store_dir: &Path) -> Resource {
    let byte_pool = harness.with_player(|player| player.byte_pool().clone());
    let config = ResourceConfig::for_src(
        ResourceConfig::parse_src(path.to_str().expect("utf-8 fixture path"))
            .expect("local media path is a valid resource src"),
    )
    .store(
        AssetStore::builder()
            .backend(StorageBackend::Disk {
                root: store_dir.into(),
            })
            .pool(byte_pool)
            .build(),
    )
    .build();
    let config = harness
        .player()
        .prepare_config(config)
        .expect("offline player remains open");
    Resource::new(config).await.expect("open local resource")
}

async fn render_blocks(harness: &OfflinePlayerHarness, blocks: usize) {
    for _ in 0..blocks {
        let _ = harness.render(BLOCK_FRAMES);
        let _ = harness.tick_and_drain();
        time::sleep(Duration::from_millis(1)).await;
    }
}

async fn media_advance(harness: &OfflinePlayerHarness, blocks: usize) -> f64 {
    let start = harness.player().position_seconds().unwrap_or(0.0);
    render_blocks(harness, blocks).await;
    harness.player().position_seconds().unwrap_or(0.0) - start
}

fn block_period() -> Duration {
    Duration::from_secs_f64(
        f64::from(u32::try_from(BLOCK_FRAMES).expect("block size fits u32"))
            / f64::from(SAMPLE_RATE),
    )
}

fn tone_magnitudes(samples: &[f32]) -> (f64, f64) {
    let mut channel = [0.0; BLOCK_FRAMES];
    for (sample, frame) in channel
        .iter_mut()
        .zip(samples.chunks_exact(usize::from(CHANNELS)))
    {
        *sample = frame[0];
    }
    (
        goertzel_magnitude(&channel, SLOW_TONE_HZ, SAMPLE_RATE),
        goertzel_magnitude(&channel, FAST_TONE_HZ, SAMPLE_RATE),
    )
}

async fn render_until_slow_tone(harness: &OfflinePlayerHarness) -> f64 {
    for _ in 0..WARMUP_BLOCK_BUDGET {
        let block = harness.render(BLOCK_FRAMES);
        let _ = harness.tick_and_drain();
        let (slow, fast) = tone_magnitudes(&block);
        if slow > 1.0 && slow > fast * TONE_DOMINANCE_RATIO {
            return slow;
        }
        time::sleep(block_period()).await;
    }
    panic!(
        "precondition: the slow-rate {SLOW_TONE_HZ} Hz output never became audible within \
         {WARMUP_BLOCK_BUDGET} blocks"
    );
}

async fn blocks_until_end(temp_dir: &TestTempDir, rate: f32) -> usize {
    let harness = OfflinePlayerHarness::with_sample_rate(
        OfflinePlayerOptions::builder()
            .crossfade_duration(0.0)
            .build(),
        SAMPLE_RATE,
    );
    let tag = format!("rate-{rate}");
    let path = temp_dir.path().join(format!("{tag}.wav"));
    let frames = DRAIN_FIXTURE_SECS * SAMPLE_RATE as usize;
    std::fs::write(&path, create_test_wav(frames, SAMPLE_RATE, 2)).expect("write wav fixture");
    let resource = file_resource(
        &harness,
        &path,
        &temp_dir.path().join(format!("store-{tag}")),
    )
    .await;
    harness.with_player(|player| {
        player.insert(resource, TrackId::allocate(), None);
        player
            .select_item(0, true)
            .expect("select first queue item");
    });
    harness.player().set_default_rate(rate);

    let mut blocks = 0usize;
    for _ in 0..DRAIN_BLOCK_BUDGET {
        let _ = harness.render(BLOCK_FRAMES);
        blocks += 1;
        let ended = harness
            .tick_and_drain()
            .iter()
            .any(|event| matches!(event, PlayerEvent::ItemDidPlayToEnd { .. }));
        if ended {
            return blocks;
        }
        time::sleep(Duration::from_millis(1)).await;
    }
    panic!("the {rate}x track never reached end-of-stream within {DRAIN_BLOCK_BUDGET} blocks");
}

#[kithara::test(tokio, multi_thread, timeout(Duration::from_secs(120)))]
async fn media_time_advances_with_the_playing_rate(temp_dir: TestTempDir) {
    let harness = OfflinePlayerHarness::with_sample_rate(
        OfflinePlayerOptions::builder()
            .crossfade_duration(0.0)
            .build(),
        SAMPLE_RATE,
    );
    let path = temp_dir.path().join("rate.mp3");
    std::fs::write(&path, EmbeddedAudio::TEST_MP3_BYTES).expect("write mp3 fixture");
    let resource = file_resource(&harness, &path, &temp_dir.path().join("store")).await;
    harness.with_player(|player| {
        player.insert(resource, TrackId::allocate(), None);
        player
            .select_item(0, true)
            .expect("select first queue item");
    });

    render_blocks(&harness, SETTLE_BLOCKS).await;
    let baseline = media_advance(&harness, MEASURE_BLOCKS).await;
    assert!(
        baseline > 0.0,
        "precondition: media time must advance at rate 1.0, got {baseline}s"
    );

    harness.player().set_default_rate(FAST_RATE);
    render_blocks(&harness, SETTLE_BLOCKS).await;
    let accelerated = media_advance(&harness, MEASURE_BLOCKS).await;

    assert!(
        accelerated >= baseline * 1.5,
        "over equal rendered-output windows media time advanced \
         {accelerated}s at rate {FAST_RATE} versus {baseline}s at rate 1.0 — \
         the reported clock is on the output scale, not the media scale"
    );
}

#[kithara::test(
    tokio,
    multi_thread,
    flash(false),
    timeout(Duration::from_secs(30)),
    hang_timeout_secs(5)
)]
#[ignore = "pins the measured 848-859 ms live-rate presentation regression; unignore when no buffering exceeds backend latency"]
#[case(StretchKind::Signalsmith, SIGNALSMITH_RESPONSE_BLOCK_BUDGET)]
#[cfg_attr(
    not(all(target_os = "windows", target_env = "msvc")),
    case(StretchKind::Bungee, BUNGEE_RESPONSE_BLOCK_BUDGET)
)]
async fn live_rate_change_reaches_presented_pcm_within_response_budget(
    temp_dir: TestTempDir,
    #[case] backend: StretchKind,
    #[case] response_block_budget: usize,
) {
    let stretch = StretchControls::new(1.0);
    stretch.set_backend(backend);
    let harness = OfflinePlayerHarness::with_sample_rate(
        OfflinePlayerOptions::builder()
            .crossfade_duration(0.0)
            .timestretch(stretch)
            .build(),
        SAMPLE_RATE,
    );
    harness.player().set_default_rate(SLOW_RATE);

    let path = temp_dir.path().join(format!("live-rate-{backend}.wav"));
    let fixture_frames =
        FIXTURE_SECONDS * usize::try_from(SAMPLE_RATE).expect("fixture sample rate fits usize");
    std::fs::write(
        &path,
        prepare_sine_wav(
            SOURCE_TONE_HZ,
            16_000,
            fixture_frames,
            SAMPLE_RATE,
            CHANNELS,
        ),
    )
    .expect("write cached sine fixture");
    let resource = file_resource(&harness, &path, &temp_dir.path().join("live-rate-store")).await;
    harness.with_player(|player| {
        player.insert(resource, TrackId::allocate(), None);
        player
            .select_item(0, true)
            .expect("select live-rate fixture");
    });

    let baseline_slow = render_until_slow_tone(&harness).await;
    time::sleep(Duration::from_millis(250)).await;
    let pre_change = harness.render(BLOCK_FRAMES);
    let _ = harness.tick_and_drain();
    let (pre_change_slow, pre_change_fast) = tone_magnitudes(&pre_change);
    assert!(
        pre_change_slow > pre_change_fast * TONE_DOMINANCE_RATIO,
        "precondition: expected buffered {SLOW_TONE_HZ} Hz audio before the command, got \
         slow={pre_change_slow:.3}, fast={pre_change_fast:.3}"
    );

    harness.player().set_default_rate(FAST_RATE);

    let mut last = (0.0, 0.0);
    let mut peak_fast = 0.0_f64;
    let mut first_fast_block = None;
    for block in 1..=RESPONSE_OBSERVATION_BLOCK_BUDGET {
        let output = harness.render(BLOCK_FRAMES);
        let _ = harness.tick_and_drain();
        last = tone_magnitudes(&output);
        peak_fast = peak_fast.max(last.1);
        if last.1 > last.0 * TONE_DOMINANCE_RATIO && last.1 > baseline_slow * 0.2 {
            first_fast_block = Some(block);
            break;
        }
        if block < RESPONSE_OBSERVATION_BLOCK_BUDGET {
            time::sleep(block_period()).await;
        }
    }

    let Some(first_fast_block) = first_fast_block else {
        panic!(
            "{backend} accepted rate {FAST_RATE}, but presented PCM never switched from \
             {SLOW_TONE_HZ} Hz to {FAST_TONE_HZ} Hz within \
             {RESPONSE_OBSERVATION_BLOCK_BUDGET} blocks: last slow={:.3}, last \
             fast={:.3}, peak fast={peak_fast:.3}",
            last.0, last.1
        );
    };
    let budget_ms = block_period().as_secs_f64()
        * f64::from(u32::try_from(response_block_budget).expect("response budget fits u32"))
        * 1_000.0;
    let actual_ms = block_period().as_secs_f64()
        * f64::from(u32::try_from(first_fast_block).expect("observed block fits u32"))
        * 1_000.0;
    assert!(
        first_fast_block <= response_block_budget,
        "{backend} accepted rate {FAST_RATE}, but presented PCM switched from \
         {SLOW_TONE_HZ} Hz to {FAST_TONE_HZ} Hz only at block {first_fast_block} \
         ({actual_ms:.1} ms), beyond the {response_block_budget}-block \
         ({budget_ms:.1} ms) response budget"
    );
}

/// The other half of the same contract: the faster media clock has to be
/// backed by the source actually draining faster. Without this, scaling the
/// clock alone would satisfy the trap above while the audio kept its speed.
#[kithara::test(tokio, multi_thread, timeout(Duration::from_secs(120)))]
async fn a_faster_rate_drains_the_real_source_sooner(temp_dir: TestTempDir) {
    let baseline = blocks_until_end(&temp_dir, 1.0).await;
    let accelerated = blocks_until_end(&temp_dir, FAST_RATE).await;

    assert!(
        accelerated * DRAIN_SHARE_DEN <= baseline * DRAIN_SHARE_NUM,
        "at rate {FAST_RATE} the source must reach end-of-stream in \
         materially fewer rendered output blocks than at rate 1.0, got \
         {accelerated} versus {baseline}"
    );
}
