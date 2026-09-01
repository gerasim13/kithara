#![cfg(not(target_arch = "wasm32"))]

use std::{f64::consts::TAU, num::NonZeroUsize};

#[cfg(feature = "perf")]
use hotpath::HotpathGuardBuilder;
use kithara::{
    platform::time::{self, Duration},
    play::{Resource, ResourceConfig, ResourceSrc},
    queue::{Queue, QueueConfig, Transition, test_utils::QueueProbe},
    warp::{StretchControls, StretchKind, WarpConfig},
};
use kithara_integration_tests::{
    TestTempDir, disk_asset_store, kithara,
    offline::{OfflinePlayerHarness, OfflinePlayerOptions},
    temp_dir,
};
use kithara_test_fixtures::{assets::signal_mp3_sine880_30s, signal::goertzel_magnitude};
use kithara_test_utils::probe::capture::{self as probe_capture, ProbeEvent};
use num_traits::AsPrimitive;

use crate::bufpool_ext::TestPools;

const SAMPLE_RATE: u32 = 44_100;
const CHANNELS: u16 = 2;
const BLOCK_FRAMES: usize = 512;
const PRE_COMMAND_FRAMES: usize = BLOCK_FRAMES * 16;
const RESPONSE_OBSERVATION_FRAMES: usize = BLOCK_FRAMES * 32;
const SLOW_RATE: f32 = 0.5;
const INTERMEDIATE_RATE: f32 = 2.0;
const FAST_RATE: f32 = 4.0;
const TONES_HZ: [f64; 3] = [440.0, 1_760.0, 3_520.0];
const TONE_DOMINANCE_RATIO: f64 = 4.0;
const TARGET_WINDOW_FRAMES: usize = 64;
const RATE_COMMAND_BURST: usize = 64;
const WARMUP_BLOCK_BUDGET: usize = 200;
const RESPONSE_BUDGET_FRAMES: usize = 441;
const RENDER_QUANTUM_FRAMES: NonZeroUsize = match NonZeroUsize::new(128) {
    Some(frames) => frames,
    None => unreachable!(),
};

const UP: RateCase = RateCase::new(SLOW_RATE, 0, INTERMEDIATE_RATE, 1, false);
const DOWN: RateCase = RateCase::new(INTERMEDIATE_RATE, 1, SLOW_RATE, 0, false);
const EXTREME: RateCase = RateCase::new(SLOW_RATE, 0, FAST_RATE, 2, false);
const BURST: RateCase = RateCase::new(SLOW_RATE, 0, INTERMEDIATE_RATE, 1, true);

#[derive(Clone, Copy, Debug)]
struct RateCase {
    initial_rate: f32,
    initial_tone: usize,
    target_rate: f32,
    target_tone: usize,
    burst: bool,
}

impl RateCase {
    const fn new(
        initial_rate: f32,
        initial_tone: usize,
        target_rate: f32,
        target_tone: usize,
        burst: bool,
    ) -> Self {
        Self {
            initial_rate,
            initial_tone,
            target_rate,
            target_tone,
            burst,
        }
    }
}

struct RateRun {
    command_frame: usize,
    probes: Vec<ProbeEvent>,
    samples: Vec<f32>,
}

fn frame_period(frames: usize) -> Duration {
    Duration::from_secs_f64(
        f64::from(u32::try_from(frames).expect("render frame count fits u32"))
            / f64::from(SAMPLE_RATE),
    )
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
    TONES_HZ.map(|tone| goertzel_magnitude(&channel[..frames], tone, SAMPLE_RATE))
}

fn tone_is_dominant(samples: &[f32], target: usize) -> bool {
    let magnitudes = tone_magnitudes(samples);
    magnitudes[target] > 1.0
        && magnitudes.iter().enumerate().all(|(index, magnitude)| {
            index == target || magnitudes[target] > magnitude * TONE_DOMINANCE_RATIO
        })
}

fn first_target_window(samples: &[f32], command_frame: usize, target: usize) -> Option<usize> {
    let channels = usize::from(CHANNELS);
    let frames = samples.len() / channels;
    (command_frame + TARGET_WINDOW_FRAMES..=frames).find_map(|end| {
        let start = end - TARGET_WINDOW_FRAMES;
        tone_is_dominant(&samples[start * channels..end * channels], target)
            .then_some(end - command_frame)
    })
}

fn command_window(run: &RateRun) -> &[f32] {
    let channels = usize::from(CHANNELS);
    let start = run.command_frame - TARGET_WINDOW_FRAMES;
    &run.samples[start * channels..run.command_frame * channels]
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
    initial_rate: f32,
) -> (OfflinePlayerHarness, Queue<TestPools>) {
    let stretch = StretchControls::new(1.0);
    stretch.set_backend(backend);
    let harness = OfflinePlayerHarness::with_sample_rate(
        OfflinePlayerOptions::builder()
            .crossfade_duration(0.0)
            .warp(
                WarpConfig::builder()
                    .stretch(stretch)
                    .render_quantum_frames(RENDER_QUANTUM_FRAMES)
                    .build(),
            )
            .build(),
        SAMPLE_RATE,
    );
    let path = signal_mp3_sine880_30s()
        .path()
        .expect("generated sine fixture is stored on disk");
    let config: ResourceConfig<TestPools> = ResourceConfig::for_src(
        ResourceSrc::parse(path.to_str().expect("utf-8 fixture path"))
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
    queue.set_default_rate(initial_rate);
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
    case: RateCase,
    issue_target: bool,
) -> RateRun {
    let (harness, queue) = playing_sine_queue(temp_dir, backend, case.initial_rate).await;
    render_until_tone(&harness, case.initial_tone).await;
    time::sleep(Duration::from_millis(250)).await;

    let mut samples = capture_frames(&harness, PRE_COMMAND_FRAMES).await;
    let command_frame = samples.len() / usize::from(CHANNELS);
    let recorder = issue_target.then(probe_capture::install);
    if issue_target {
        if case.burst {
            for command in 0..RATE_COMMAND_BURST {
                queue.set_rate(if command.is_multiple_of(2) {
                    FAST_RATE
                } else {
                    SLOW_RATE
                });
            }
        }
        queue.set_rate(case.target_rate);
    }
    samples.extend(capture_frames(&harness, RESPONSE_OBSERVATION_FRAMES).await);
    let probes = recorder.map_or_else(Vec::new, |recorder| recorder.snapshot());

    RateRun {
        command_frame,
        probes,
        samples,
    }
}

fn assert_response(
    backend: StretchKind,
    label: &str,
    case: RateCase,
    candidate: &RateRun,
    control: &RateRun,
) {
    assert!(
        tone_is_dominant(command_window(candidate), case.initial_tone),
        "{backend} {label} candidate was not playing the initial tone before set_rate"
    );
    assert!(
        tone_is_dominant(command_window(control), case.initial_tone),
        "{backend} {label} no-command control was not playing the initial tone"
    );
    assert!(
        !tone_is_dominant(command_window(candidate), case.target_tone),
        "{backend} {label} candidate already contained the target tone before set_rate"
    );
    assert!(
        first_target_window(&control.samples, control.command_frame, case.initial_tone).is_some(),
        "{backend} {label} no-command control stopped producing the initial tone"
    );
    assert!(
        first_target_window(&control.samples, control.command_frame, case.target_tone).is_none(),
        "{backend} {label} no-command control already contains the target tone"
    );

    let target_end = first_target_window(
        &candidate.samples,
        candidate.command_frame,
        case.target_tone,
    )
    .unwrap_or_else(|| {
        panic!(
            "{backend} {label} command never produced dominant {} Hz PCM",
            TONES_HZ[case.target_tone]
        )
    });
    let requested = candidate
        .probes
        .iter()
        .filter(|event| event.probe_name() == Some("rate_requested"))
        .max_by_key(|event| event.seq().unwrap_or(0))
        .unwrap_or_else(|| panic!("{backend} {label} emitted no rate_requested probe"));
    let revision = requested
        .u64("request_revision")
        .unwrap_or_else(|| panic!("{backend} {label} request probe has no revision"));
    let target_rate_bits = requested
        .u64("target_rate_bits")
        .unwrap_or_else(|| panic!("{backend} {label} request probe has no target rate"));
    let session_epoch = requested
        .u64("session_epoch")
        .unwrap_or_else(|| panic!("{backend} {label} request probe has no session epoch"));
    let request_frame = requested
        .u64("presentation_frame")
        .and_then(|frame| i64::try_from(frame).ok())
        .unwrap_or_else(|| panic!("{backend} {label} request probe has no presentation frame"));
    assert_eq!(
        target_rate_bits,
        u64::from(case.target_rate.to_bits()),
        "{backend} {label} correlated the wrong final rate request"
    );

    let applied = candidate
        .probes
        .iter()
        .filter(|event| event.probe_name() == Some("rate_applied"))
        .filter(|event| event.u64("request_revision") == Some(revision))
        .filter(|event| event.u64("target_rate_bits") == Some(target_rate_bits))
        .filter(|event| event.u64("session_epoch") == Some(session_epoch))
        .filter_map(|event| {
            event
                .u64("session_frame")
                .and_then(|frame| i64::try_from(frame).ok())
        })
        .min()
        .unwrap_or_else(|| {
            panic!(
                "{backend} {label} emitted no successful rate_applied probe for revision \
                 {revision}"
            )
        });
    let response_frames = applied.checked_sub(request_frame).unwrap_or_else(|| {
        panic!(
            "{backend} {label} applied revision {revision} at frame {applied} before its request \
             boundary {request_frame}"
        )
    });
    assert!(
        response_frames <= i64::try_from(RESPONSE_BUDGET_FRAMES).unwrap_or(i64::MAX),
        "{backend} {label} applied revision {revision} after {response_frames} presented frames; \
         hard budget is {RESPONSE_BUDGET_FRAMES} frames"
    );
    assert!(
        target_end <= RESPONSE_BUDGET_FRAMES,
        "{backend} {label} target {} Hz became dominant at {target_end} frames after revision \
         {revision} first applied at {response_frames} frames; hard budget is \
         {RESPONSE_BUDGET_FRAMES} frames",
        TONES_HZ[case.target_tone],
    );
}

async fn run_response_case(
    temp_dir: &TestTempDir,
    backend: StretchKind,
    case: RateCase,
    label: &str,
) {
    let control = response_run(temp_dir, backend, case, false).await;

    #[cfg(feature = "perf")]
    let _guard = HotpathGuardBuilder::new("live_rate_response").build();

    let candidate = response_run(temp_dir, backend, case, true).await;
    assert_response(backend, label, case, &candidate, &control);
}

#[kithara::test(
    tokio,
    multi_thread,
    serial,
    flash(false),
    timeout(Duration::from_secs(30)),
    hang_timeout_secs(5)
)]
#[case::signalsmith_up(StretchKind::Signalsmith, UP)]
#[case::signalsmith_down(StretchKind::Signalsmith, DOWN)]
#[case::signalsmith_extreme(StretchKind::Signalsmith, EXTREME)]
#[cfg_attr(
    not(all(target_os = "windows", target_env = "msvc")),
    case::bungee_up(StretchKind::Bungee, UP)
)]
#[cfg_attr(
    not(all(target_os = "windows", target_env = "msvc")),
    case::bungee_down(StretchKind::Bungee, DOWN)
)]
#[cfg_attr(
    not(all(target_os = "windows", target_env = "msvc")),
    case::bungee_extreme(StretchKind::Bungee, EXTREME)
)]
async fn live_rate_change_reaches_presented_pcm_within_response_budget(
    temp_dir: TestTempDir,
    #[case] backend: StretchKind,
    #[case] case: RateCase,
) {
    run_response_case(&temp_dir, backend, case, "live rate change").await;
}

#[kithara::test(
    tokio,
    multi_thread,
    serial,
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
    run_response_case(&temp_dir, backend, BURST, "latest-wins burst").await;
}

#[kithara::test(native, flash(false))]
fn target_window_distinguishes_every_rate_across_phase() {
    let channels = usize::from(CHANNELS);
    for (target, tone) in TONES_HZ.into_iter().enumerate() {
        for phase_step in 0..128_u32 {
            let phase = TAU * f64::from(phase_step) / 128.0;
            let mut samples = Vec::with_capacity(TARGET_WINDOW_FRAMES * channels);
            for frame in 0..TARGET_WINDOW_FRAMES {
                let frame = f64::from(u32::try_from(frame).expect("tone window fits u32"));
                let sample: f32 = (phase + TAU * tone * frame / f64::from(SAMPLE_RATE))
                    .sin()
                    .as_();
                samples.extend(std::iter::repeat_n(sample, channels));
            }
            assert!(
                tone_is_dominant(&samples, target),
                "{tone} Hz target is ambiguous at phase step {phase_step}"
            );
            for other in 0..TONES_HZ.len() {
                if other != target {
                    assert!(
                        !tone_is_dominant(&samples, other),
                        "{tone} Hz was misclassified as {} Hz at phase step {phase_step}",
                        TONES_HZ[other]
                    );
                }
            }
        }
    }
}
