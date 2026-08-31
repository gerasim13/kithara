#![cfg(not(target_arch = "wasm32"))]

use std::num::NonZeroU32;

#[cfg(feature = "perf")]
use hotpath::HotpathGuardBuilder;
use kithara::{
    platform::time::{self, Duration},
    play::{Resource, ResourceConfig},
    queue::{Queue, QueueConfig, Transition, test_utils::QueueProbe},
    signal::AudioSpec,
    warp::{StretchControls, StretchKind},
};
use kithara_integration_tests::{
    TestTempDir,
    cochlea::{align_command_runs, first_sustained_delta},
    disk_asset_store, kithara,
    offline::{OfflinePlayerHarness, OfflinePlayerOptions},
    temp_dir,
};
use kithara_test_fixtures::{assets::signal_mp3_sine880_30s, signal::goertzel_magnitude};

const SAMPLE_RATE: u32 = 44_100;
const CHANNELS: u16 = 2;
const BLOCK_FRAMES: usize = 512;
const PRE_COMMAND_FRAMES: usize = BLOCK_FRAMES * 8;
const RESPONSE_OBSERVATION_FRAMES: usize = BLOCK_FRAMES * 32;
const SLOW_RATE: f32 = 0.5;
const INTERMEDIATE_RATE: f32 = 2.0;
const FAST_RATE: f32 = 4.0;
const SLOW_TONE_HZ: f64 = 440.0;
const INTERMEDIATE_TONE_HZ: f64 = 1_760.0;
const FAST_TONE_HZ: f64 = 3_520.0;
const TONE_DOMINANCE_RATIO: f64 = 4.0;
const RATE_COMMAND_BURST: usize = 64;
const WARMUP_BLOCK_BUDGET: usize = 200;
const RESPONSE_BUDGET: Duration = Duration::from_millis(10);
const RESPONSE_DELTA_THRESHOLD: f32 = 0.002;
const RESPONSE_SUSTAINED_FRAMES: usize = 32;

struct RateRun {
    command_frame: usize,
    samples: Vec<f32>,
}

fn frame_period(frames: usize) -> Duration {
    Duration::from_secs_f64(
        f64::from(u32::try_from(frames).expect("render frame count fits u32"))
            / f64::from(SAMPLE_RATE),
    )
}

fn response_frame_budget() -> usize {
    AudioSpec::new(
        CHANNELS,
        NonZeroU32::new(SAMPLE_RATE).expect("fixture sample rate is non-zero"),
    )
    .frames_for(RESPONSE_BUDGET)
    .expect("response budget fits the fixture sample rate")
    .get()
}

fn tone_magnitudes(samples: &[f32]) -> [f64; 3] {
    let frames = samples.len() / usize::from(CHANNELS);
    assert!(
        frames <= BLOCK_FRAMES,
        "tone probe exceeds its fixed window"
    );
    let mut channel = [0.0; BLOCK_FRAMES];
    for (sample, frame) in channel[..frames]
        .iter_mut()
        .zip(samples.chunks_exact(usize::from(CHANNELS)))
    {
        *sample = frame[0];
    }
    [
        goertzel_magnitude(&channel[..frames], SLOW_TONE_HZ, SAMPLE_RATE),
        goertzel_magnitude(&channel[..frames], INTERMEDIATE_TONE_HZ, SAMPLE_RATE),
        goertzel_magnitude(&channel[..frames], FAST_TONE_HZ, SAMPLE_RATE),
    ]
}

fn tone_is_dominant(samples: &[f32], target: usize) -> bool {
    let magnitudes = tone_magnitudes(samples);
    magnitudes[target] > 1.0
        && magnitudes.iter().enumerate().all(|(index, magnitude)| {
            index == target || magnitudes[target] > magnitude * TONE_DOMINANCE_RATIO
        })
}

async fn render_until_tone(harness: &OfflinePlayerHarness, target: usize) {
    for _ in 0..WARMUP_BLOCK_BUDGET {
        let block = harness.render(BLOCK_FRAMES);
        let _ = harness.tick_and_drain();
        if tone_is_dominant(&block, target) {
            return;
        }
        time::sleep(frame_period(BLOCK_FRAMES)).await;
    }
    panic!("precondition: target tone {target} never became audible");
}

async fn playing_sine_queue(
    temp_dir: &TestTempDir,
    backend: StretchKind,
) -> (OfflinePlayerHarness, Queue) {
    let stretch = StretchControls::new(1.0);
    stretch.set_backend(backend);
    let harness = OfflinePlayerHarness::with_sample_rate(
        OfflinePlayerOptions::builder()
            .crossfade_duration(0.0)
            .timestretch(stretch)
            .build(),
        SAMPLE_RATE,
    );
    let path = signal_mp3_sine880_30s()
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
    let queue = Queue::new(
        QueueConfig::builder()
            .player(harness.take_player())
            .should_autoplay(false)
            .build(),
    );
    queue.set_default_rate(SLOW_RATE);
    let id = queue.insert_loaded_for_test(resource);
    queue
        .select(id, Transition::None)
        .expect("select live-rate fixture");
    (harness, queue)
}

async fn capture_frames(harness: &OfflinePlayerHarness, frames: usize) -> Vec<f32> {
    let mut samples = Vec::with_capacity(frames * usize::from(CHANNELS));
    while samples.len() / usize::from(CHANNELS) < frames {
        let remaining = frames - samples.len() / usize::from(CHANNELS);
        let block_frames = remaining.min(BLOCK_FRAMES);
        samples.extend(harness.render(block_frames));
        let _ = harness.tick_and_drain();
        time::sleep(frame_period(block_frames)).await;
    }
    samples
}

async fn response_run(
    temp_dir: &TestTempDir,
    backend: StretchKind,
    two_step: bool,
    burst: bool,
    issue_target: bool,
) -> RateRun {
    let (harness, queue) = playing_sine_queue(temp_dir, backend).await;
    render_until_tone(&harness, 0).await;
    if two_step {
        queue.set_rate(INTERMEDIATE_RATE);
        render_until_tone(&harness, 1).await;
    }
    time::sleep(Duration::from_millis(250)).await;

    let mut samples = capture_frames(&harness, PRE_COMMAND_FRAMES).await;
    let command_frame = samples.len() / usize::from(CHANNELS);
    if burst {
        let current_rate = if two_step {
            INTERMEDIATE_RATE
        } else {
            SLOW_RATE
        };
        for _ in 0..RATE_COMMAND_BURST {
            queue.set_rate(current_rate);
        }
        if !issue_target {
            queue.set_rate(current_rate);
        }
    }
    if issue_target {
        queue.set_rate(FAST_RATE);
    }
    samples.extend(capture_frames(&harness, RESPONSE_OBSERVATION_FRAMES).await);

    RateRun {
        command_frame,
        samples,
    }
}

fn eventually_reaches_fast_tone(samples: &[f32], command_frame: usize) -> bool {
    samples[command_frame * usize::from(CHANNELS)..]
        .chunks_exact(BLOCK_FRAMES * usize::from(CHANNELS))
        .any(|block| tone_is_dominant(block, 2))
}

fn assert_response(backend: StretchKind, label: &str, candidate: &RateRun, control: &RateRun) {
    let aligned = align_command_runs(
        &candidate.samples,
        candidate.command_frame,
        &control.samples,
        control.command_frame,
        CHANNELS,
    );
    let frames = aligned.candidate.len() / usize::from(CHANNELS);
    assert!(
        first_sustained_delta(
            &aligned.candidate,
            &aligned.control,
            CHANNELS,
            0..aligned.command_frame,
            RESPONSE_DELTA_THRESHOLD,
            RESPONSE_SUSTAINED_FRAMES,
        )
        .is_none(),
        "{backend} {label} candidate diverged from its no-command control before set_rate"
    );
    assert!(
        eventually_reaches_fast_tone(&aligned.candidate, aligned.command_frame),
        "{backend} {label} command never produced dominant {FAST_TONE_HZ} Hz PCM"
    );

    let transition = first_sustained_delta(
        &aligned.candidate,
        &aligned.control,
        CHANNELS,
        aligned.command_frame..frames,
        RESPONSE_DELTA_THRESHOLD,
        RESPONSE_SUSTAINED_FRAMES,
    );
    let observed = transition.map(|frame| frame - aligned.command_frame);
    let budget = response_frame_budget();
    assert!(
        observed.is_some_and(|frames| frames <= budget),
        "{backend} {label} changed presented PCM at {observed:?} frames; hard budget is {budget} \
         frames ({:.1} ms)",
        RESPONSE_BUDGET.as_secs_f64() * 1_000.0,
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
#[case::signalsmith_two_step(StretchKind::Signalsmith, true)]
#[cfg_attr(
    not(all(target_os = "windows", target_env = "msvc")),
    case::bungee_direct(StretchKind::Bungee, false)
)]
#[cfg_attr(
    not(all(target_os = "windows", target_env = "msvc")),
    case::bungee_two_step(StretchKind::Bungee, true)
)]
async fn live_rate_change_reaches_presented_pcm_within_response_budget(
    temp_dir: TestTempDir,
    #[case] backend: StretchKind,
    #[case] two_step: bool,
) {
    let control = response_run(&temp_dir, backend, two_step, false, false).await;

    #[cfg(feature = "perf")]
    let _guard = HotpathGuardBuilder::new("live_rate_response").build();

    let candidate = response_run(&temp_dir, backend, two_step, false, true).await;
    assert_response(backend, "live rate change", &candidate, &control);
}

#[kithara::test(
    tokio,
    multi_thread,
    flash(false),
    timeout(Duration::from_secs(30)),
    hang_timeout_secs(5)
)]
#[case::signalsmith(StretchKind::Signalsmith)]
#[cfg_attr(
    not(all(target_os = "windows", target_env = "msvc")),
    case::bungee(StretchKind::Bungee)
)]
async fn latest_rate_wins_after_a_control_burst(
    temp_dir: TestTempDir,
    #[case] backend: StretchKind,
) {
    let control = response_run(&temp_dir, backend, false, true, false).await;
    let candidate = response_run(&temp_dir, backend, false, true, true).await;
    assert_response(backend, "latest-wins burst", &candidate, &control);
}
