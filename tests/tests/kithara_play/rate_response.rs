#![cfg(not(target_arch = "wasm32"))]

#[cfg(feature = "perf")]
use hotpath::HotpathGuardBuilder;
use kithara::{
    events::TrackId,
    platform::time::{self, Duration},
    play::{Resource, ResourceConfig},
    warp::{StretchControls, StretchKind},
};
use kithara_integration_tests::{
    TestTempDir, disk_asset_store, kithara,
    offline::{OfflinePlayerHarness, OfflinePlayerOptions},
    temp_dir,
};
use kithara_test_fixtures::{assets::signal_wav_sine440_60s, signal::goertzel_magnitude};

const SAMPLE_RATE: u32 = 44_100;
const CHANNELS: u16 = 2;
const BLOCK_FRAMES: usize = 512;
const SLOW_RATE: f32 = 0.5;
const FAST_RATE: f32 = 2.0;
const SLOW_TONE_HZ: f64 = 220.0;
const UNITY_TONE_HZ: f64 = 440.0;
const FAST_TONE_HZ: f64 = 880.0;
const TONE_DOMINANCE_RATIO: f64 = 4.0;
const WARMUP_BLOCK_BUDGET: usize = 200;
const RESPONSE_BLOCK_BUDGET: usize = 15;
const RESPONSE_OBSERVATION_BLOCK_BUDGET: usize = 400;

fn block_period() -> Duration {
    Duration::from_secs_f64(
        f64::from(u32::try_from(BLOCK_FRAMES).expect("block size fits u32"))
            / f64::from(SAMPLE_RATE),
    )
}

fn tone_magnitudes(samples: &[f32]) -> (f64, f64, f64) {
    let mut channel = [0.0; BLOCK_FRAMES];
    for (sample, frame) in channel
        .iter_mut()
        .zip(samples.chunks_exact(usize::from(CHANNELS)))
    {
        *sample = frame[0];
    }
    (
        goertzel_magnitude(&channel, SLOW_TONE_HZ, SAMPLE_RATE),
        goertzel_magnitude(&channel, UNITY_TONE_HZ, SAMPLE_RATE),
        goertzel_magnitude(&channel, FAST_TONE_HZ, SAMPLE_RATE),
    )
}

async fn render_until_slow_tone(harness: &OfflinePlayerHarness) -> f64 {
    for _ in 0..WARMUP_BLOCK_BUDGET {
        let block = harness.render(BLOCK_FRAMES);
        let _ = harness.tick_and_drain();
        let (slow, unity, fast) = tone_magnitudes(&block);
        if slow > 1.0 && slow > unity * TONE_DOMINANCE_RATIO && slow > fast * TONE_DOMINANCE_RATIO {
            return slow;
        }
        time::sleep(block_period()).await;
    }
    panic!(
        "precondition: the slow-rate {SLOW_TONE_HZ} Hz output never became audible within \
         {WARMUP_BLOCK_BUDGET} blocks"
    );
}

#[kithara::test(
    tokio,
    multi_thread,
    flash(false),
    timeout(Duration::from_secs(30)),
    hang_timeout_secs(5)
)]
#[case::signalsmith_direct(StretchKind::Signalsmith, false)]
#[case::signalsmith_through_unity(StretchKind::Signalsmith, true)]
#[cfg_attr(
    not(all(target_os = "windows", target_env = "msvc")),
    case::bungee_direct(StretchKind::Bungee, false)
)]
#[cfg_attr(
    not(all(target_os = "windows", target_env = "msvc")),
    case::bungee_through_unity(StretchKind::Bungee, true)
)]
async fn live_rate_change_reaches_presented_pcm_within_response_budget(
    temp_dir: TestTempDir,
    #[case] backend: StretchKind,
    #[case] through_unity: bool,
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

    let path = signal_wav_sine440_60s()
        .path()
        .expect("generated sine fixture is stored on disk");
    let config = ResourceConfig::for_src(
        ResourceConfig::parse_src(path.to_str().expect("utf-8 fixture path"))
            .expect("local media path is a valid resource src"),
    )
    .store(disk_asset_store(temp_dir.path().join("live-rate-store")))
    .build();
    let config = harness
        .player()
        .prepare_config(config)
        .expect("offline player remains open");
    let resource = Resource::new(config).await.expect("open local resource");
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
    let (pre_change_slow, pre_change_unity, pre_change_fast) = tone_magnitudes(&pre_change);
    assert!(
        pre_change_slow > pre_change_unity * TONE_DOMINANCE_RATIO
            && pre_change_slow > pre_change_fast * TONE_DOMINANCE_RATIO,
        "precondition: expected buffered {SLOW_TONE_HZ} Hz audio before the command, got \
         slow={pre_change_slow:.3}, unity={pre_change_unity:.3}, fast={pre_change_fast:.3}"
    );

    #[cfg(feature = "perf")]
    let _guard = HotpathGuardBuilder::new("live_rate_response").build();

    if through_unity {
        harness.player().set_default_rate(1.0);
        let mut presented_unity = false;
        for block in 1..=RESPONSE_BLOCK_BUDGET {
            let output = harness.render(BLOCK_FRAMES);
            let _ = harness.tick_and_drain();
            let (slow, unity, fast) = tone_magnitudes(&output);
            if unity > baseline_slow * 0.2
                && unity > slow * TONE_DOMINANCE_RATIO
                && unity > fast * TONE_DOMINANCE_RATIO
            {
                presented_unity = true;
                break;
            }
            if block < RESPONSE_BLOCK_BUDGET {
                time::sleep(block_period()).await;
            }
        }
        assert!(
            presented_unity,
            "{backend} accepted unity rate, but presented PCM did not reach dominant \
             {UNITY_TONE_HZ} Hz within the common {RESPONSE_BLOCK_BUDGET}-block budget"
        );
    }
    harness.player().set_default_rate(FAST_RATE);

    let mut last = (0.0, 0.0, 0.0);
    let mut peak_fast = 0.0_f64;
    let mut first_fast_block = None;
    for block in 1..=RESPONSE_OBSERVATION_BLOCK_BUDGET {
        let output = harness.render(BLOCK_FRAMES);
        let _ = harness.tick_and_drain();
        last = tone_magnitudes(&output);
        peak_fast = peak_fast.max(last.2);
        if last.2 > last.0 * TONE_DOMINANCE_RATIO
            && last.2 > last.1 * TONE_DOMINANCE_RATIO
            && last.2 > baseline_slow * 0.2
        {
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
             {RESPONSE_OBSERVATION_BLOCK_BUDGET} blocks: last slow={:.3}, last unity={:.3}, \
             last fast={:.3}, peak fast={peak_fast:.3}",
            last.0, last.1, last.2
        );
    };
    let budget_ms = block_period().as_secs_f64()
        * f64::from(u32::try_from(RESPONSE_BLOCK_BUDGET).expect("response budget fits u32"))
        * 1_000.0;
    let actual_ms = block_period().as_secs_f64()
        * f64::from(u32::try_from(first_fast_block).expect("observed block fits u32"))
        * 1_000.0;
    assert!(
        first_fast_block <= RESPONSE_BLOCK_BUDGET,
        "{backend} accepted rate {FAST_RATE} (through_unity={through_unity}), but presented PCM switched from \
         {SLOW_TONE_HZ} Hz to {FAST_TONE_HZ} Hz only at block {first_fast_block} \
         ({actual_ms:.1} ms), beyond the {RESPONSE_BLOCK_BUDGET}-block \
         ({budget_ms:.1} ms) response budget"
    );
}
