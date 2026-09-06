#![cfg(not(target_arch = "wasm32"))]

use std::num::{NonZeroU32, NonZeroUsize};

use kithara::{
    host::HostOwned,
    platform::time::{self, Duration},
    play::{Resource, ResourceConfig, ResourceSrc},
    queue::{Queue, QueueConfig, Transition, test_utils::QueueProbe},
    stretch::ElasticBackendConfig,
    warp::{StretchControls, StretchKind, WarpConfig},
};
use kithara_integration_tests::{
    TestTempDir, disk_asset_store, kithara,
    offline::{OfflinePlayerHarness, OfflinePlayerOptions},
    temp_dir,
};
use kithara_test_fixtures::{assets::signal_mp3_sine880_30s, signal::goertzel_magnitude};
use kithara_test_utils::probe::capture::{self as probe_capture, ProbeEvent, Recorder};

use crate::bufpool_ext::TestPools;

const SAMPLE_RATE: u32 = 44_100;
const CHANNELS: u16 = 2;
const TARGET_WINDOW_FRAMES: usize = 128;
const TONES_HZ: [f64; 4] = [440.0, 880.0, 1_760.0, 3_520.0];
const TONE_DOMINANCE_RATIO: f64 = 4.0;
const MIN_SIGNAL_RMS: f64 = 0.003;
const WARMUP_BLOCK_BUDGET: usize = 200;

const MINIMUM: ResponseCase = ResponseCase::new(128, 4_096, 16, 1, 441, 1.0, 1, 2.0, 2, 0);
const PRODUCT: ResponseCase = ResponseCase::new(128, 8_192, 32, 12, 441, 2.0, 2, 0.5, 0, 0);
const EXTREME: ResponseCase = ResponseCase::new(512, 16_384, 64, 64, 441, 0.5, 0, 4.0, 3, 64);

#[derive(Clone, Copy, Debug)]
struct ResponseCase {
    callback_frames: usize,
    source_block_frames: usize,
    render_quantum_frames: usize,
    smooth_frames: usize,
    response_budget_frames: usize,
    initial_rate: f32,
    initial_tone: usize,
    target_rate: f32,
    target_tone: usize,
    burst: usize,
}

impl ResponseCase {
    const fn new(
        callback_frames: usize,
        source_block_frames: usize,
        render_quantum_frames: usize,
        smooth_frames: usize,
        response_budget_frames: usize,
        initial_rate: f32,
        initial_tone: usize,
        target_rate: f32,
        target_tone: usize,
        burst: usize,
    ) -> Self {
        Self {
            callback_frames,
            source_block_frames,
            render_quantum_frames,
            smooth_frames,
            response_budget_frames,
            initial_rate,
            initial_tone,
            target_rate,
            target_tone,
            burst,
        }
    }

    fn observation_frames(self) -> usize {
        self.response_budget_frames
            .saturating_add(self.source_block_frames.saturating_mul(2))
            .saturating_add(TARGET_WINDOW_FRAMES)
            .saturating_add(self.callback_frames.saturating_mul(2))
    }
}

fn frame_period(frames: usize) -> Duration {
    Duration::from_secs_f64(
        f64::from(u32::try_from(frames).expect("callback frame count fits u32"))
            / f64::from(SAMPLE_RATE),
    )
}

fn signal_rms(samples: &[f32]) -> f64 {
    let mut energy = 0.0;
    let mut frames = 0_u32;
    for frame in samples.chunks_exact(usize::from(CHANNELS)) {
        energy = f64::from(frame[0]).mul_add(f64::from(frame[0]), energy);
        frames += 1;
    }
    if frames == 0 {
        return 0.0;
    }
    (energy / f64::from(frames)).sqrt()
}

fn tone_is_dominant(samples: &[f32], target: usize) -> bool {
    let mut channel = [0.0; TARGET_WINDOW_FRAMES];
    let mut frames = 0;
    for (sample, frame) in channel
        .iter_mut()
        .zip(samples.chunks_exact(usize::from(CHANNELS)))
    {
        *sample = frame[0];
        frames += 1;
    }
    let magnitudes = TONES_HZ.map(|tone| goertzel_magnitude(&channel[..frames], tone, SAMPLE_RATE));
    signal_rms(samples) >= MIN_SIGNAL_RMS
        && magnitudes.iter().enumerate().all(|(index, magnitude)| {
            index == target || magnitudes[target] > magnitude * TONE_DOMINANCE_RATIO
        })
}

fn first_target_onset(samples: &[f32], command_frame: usize, target: usize) -> Option<usize> {
    let channels = usize::from(CHANNELS);
    let frames = samples.len() / channels;
    (command_frame + TARGET_WINDOW_FRAMES..=frames).find_map(|end| {
        let start = end - TARGET_WINDOW_FRAMES;
        tone_is_dominant(&samples[start * channels..end * channels], target)
            .then_some(start - command_frame)
    })
}

async fn capture_frames(
    harness: &OfflinePlayerHarness,
    frames: usize,
    callback_frames: usize,
) -> Vec<f32> {
    let mut samples = Vec::with_capacity(frames * usize::from(CHANNELS));
    while samples.len() / usize::from(CHANNELS) < frames {
        let remaining = frames - samples.len() / usize::from(CHANNELS);
        let block_frames = remaining.min(callback_frames);
        samples.extend(harness.render(block_frames));
        let _ = harness.tick_and_drain();
        time::sleep(frame_period(block_frames)).await;
    }
    samples
}

async fn capture_command_boundary(
    harness: &OfflinePlayerHarness,
    recorder: &Recorder,
    target: usize,
    callback_frames: usize,
) -> Vec<f32> {
    let mut publish_seq = latest_probe_seq(&recorder.snapshot(), "publish");
    for _ in 0..WARMUP_BLOCK_BUDGET {
        let block = capture_frames(harness, callback_frames, callback_frames).await;
        let after = recorder.snapshot();
        let current_publish_seq = latest_probe_seq(&after, "publish");
        let published = current_publish_seq > publish_seq;
        publish_seq = current_publish_seq;
        if published && tone_is_dominant(&block, target) {
            return block;
        }
    }
    panic!("precondition: no callback presented the initial tone at a transport boundary");
}

fn latest_probe_seq(events: &[ProbeEvent], name: &str) -> Option<u64> {
    events
        .iter()
        .filter(|event| event.probe_name() == Some(name))
        .filter_map(ProbeEvent::seq)
        .max()
}

async fn playing_queue(
    temp_dir: &TestTempDir,
    backend: StretchKind,
    backends: ElasticBackendConfig,
    case: ResponseCase,
) -> (OfflinePlayerHarness, HostOwned<Queue<TestPools>>) {
    let stretch = StretchControls::new(1.0);
    stretch.set_backend(backend);
    let warp = WarpConfig::builder()
        .stretch(stretch)
        .backends(backends)
        .source_block_frames(
            NonZeroUsize::new(case.source_block_frames).expect("case source block is non-zero"),
        )
        .rate_smooth_frames(
            NonZeroUsize::new(case.smooth_frames).expect("case smoothing is non-zero"),
        )
        .render_quantum_frames(
            NonZeroUsize::new(case.render_quantum_frames).expect("case quantum is non-zero"),
        )
        .build();
    let harness = OfflinePlayerHarness::with_sample_rate(
        OfflinePlayerOptions::builder()
            .crossfade_duration(0.0)
            .warp(warp)
            .output_block_frames(
                NonZeroU32::new(
                    u32::try_from(case.callback_frames).expect("case callback fits u32"),
                )
                .expect("case callback is non-zero"),
            )
            .build(),
        SAMPLE_RATE,
    );
    let path = signal_mp3_sine880_30s()
        .path()
        .expect("generated sine fixture is stored on disk");
    let config: ResourceConfig<TestPools> = ResourceConfig::for_src(
        ResourceSrc::parse(path.to_str().expect("utf-8 fixture path"))
            .expect("fixture path is a valid resource source"),
    )
    .store(disk_asset_store(
        temp_dir.path().join("rate-response-store"),
    ))
    .build();
    let config = harness
        .player()
        .prepare_config(config)
        .expect("offline player remains open");
    let resource = Resource::new(config).await.expect("open sine fixture");
    let queue = Queue::new(
        QueueConfig::builder()
            .player(harness.take_player())
            .should_autoplay(false)
            .build(),
    );
    let queue = harness.insert(queue);
    queue.set_default_rate(case.initial_rate);
    let id = queue.insert_loaded_for_test(resource);
    queue
        .select(id, Transition::None)
        .expect("select live-rate fixture");
    (harness, queue)
}

fn revision_probe<'a>(
    events: &'a [ProbeEvent],
    name: &str,
    field: &str,
    revision: u64,
) -> Option<&'a ProbeEvent> {
    events
        .iter()
        .filter(|event| event.probe_name() == Some(name))
        .filter(|event| event.u64(field) == Some(revision))
        .min_by_key(|event| event.seq().unwrap_or(u64::MAX))
}

fn assert_response(
    backend: StretchKind,
    case: ResponseCase,
    command_frame: usize,
    samples: &[f32],
    events: &[ProbeEvent],
) {
    let requested = events
        .iter()
        .filter(|event| event.probe_name() == Some("rate_requested"))
        .max_by_key(|event| event.seq().unwrap_or(0))
        .unwrap_or_else(|| {
            let publish = events
                .iter()
                .filter(|event| event.probe_name() == Some("publish"))
                .count();
            let consumed = events
                .iter()
                .filter(|event| event.probe_name() == Some("pcm_consumed"))
                .count();
            let applied = events
                .iter()
                .filter(|event| event.probe_name() == Some("rate_applied"))
                .count();
            panic!(
                "{backend} emitted no rate_requested probe; publish={publish}, rate_applied={applied}, pcm_consumed={consumed}"
            )
        });
    let revision = requested
        .u64("request_revision")
        .unwrap_or_else(|| panic!("{backend} request probe has no revision"));
    assert_eq!(
        requested.u64("target_rate_bits"),
        Some(u64::from(case.target_rate.to_bits())),
        "{backend} correlated the wrong final rate request"
    );
    let request_frame = requested
        .u64("session_frame")
        .and_then(|frame| i64::try_from(frame).ok())
        .unwrap_or_else(|| panic!("{backend} request probe has no session frame"));
    let budget = i64::try_from(case.response_budget_frames).expect("case budget fits i64");
    let observed_frames = samples
        .len()
        .checked_div(usize::from(CHANNELS))
        .and_then(|frames| frames.checked_sub(command_frame))
        .expect("captured output includes the command boundary");
    let applied = revision_probe(events, "rate_applied", "request_revision", revision)
        .unwrap_or_else(|| {
            panic!(
                "{backend} did not apply revision {revision} within {observed_frames} rendered output frames; response budget is {budget}"
            )
        });
    let consumed = revision_probe(events, "pcm_consumed", "render_revision", revision)
        .unwrap_or_else(|| panic!("{backend} presented no PCM for {revision}"));
    let applied_frame = applied
        .u64("session_frame")
        .and_then(|frame| i64::try_from(frame).ok())
        .unwrap_or_else(|| panic!("{backend} apply probe has no session frame"));
    let consumed_frame = consumed
        .u64("output_start")
        .and_then(|frame| i64::try_from(frame).ok())
        .unwrap_or_else(|| panic!("{backend} PCM probe has no output start"));
    let applied_response = applied_frame
        .checked_sub(request_frame)
        .unwrap_or_else(|| panic!("{backend} applied revision before its request"));
    let presented_response = consumed_frame
        .checked_sub(request_frame)
        .unwrap_or_else(|| panic!("{backend} presented revision before its request"));
    assert!(
        applied_response <= budget,
        "{backend} applied revision {revision} after {applied_response} frames; budget is {budget}"
    );
    assert!(
        presented_response <= budget,
        "{backend} presented revision {revision} after {presented_response} frames; budget is {budget}"
    );
    let onset = first_target_onset(samples, command_frame, case.target_tone)
        .unwrap_or_else(|| panic!("{backend} never produced the target tone"));
    let primed = revision_probe(events, "prime_activation", "request_revision", revision)
        .map(|event| (event.u64("source_frames"), event.u64("output_frames")));
    assert!(
        onset <= case.response_budget_frames,
        "{backend} target tone began after {onset} frames; budget is {}; primed={primed:?}",
        case.response_budget_frames
    );
}

async fn run_case(
    temp_dir: &TestTempDir,
    backend: StretchKind,
    backends: ElasticBackendConfig,
    case: ResponseCase,
) {
    let (harness, queue) = playing_queue(temp_dir, backend, backends, case).await;
    let recorder = probe_capture::install();
    let mut samples =
        capture_command_boundary(&harness, &recorder, case.initial_tone, case.callback_frames)
            .await;
    let command_frame = samples.len() / usize::from(CHANNELS);
    assert!(
        queue.is_playing(),
        "{backend} command boundary is not playing"
    );
    let ready = recorder.snapshot();
    let published_end = ready
        .iter()
        .filter(|event| event.probe_name() == Some("publish"))
        .max_by_key(|event| event.seq().unwrap_or(0))
        .and_then(|event| event.u64("output_end"));
    let consumed_end = ready
        .iter()
        .filter(|event| event.probe_name() == Some("pcm_consumed"))
        .max_by_key(|event| event.seq().unwrap_or(0))
        .and_then(|event| event.u64("output_end"));
    let published_end = published_end
        .unwrap_or_else(|| panic!("{backend} command boundary has no published transport"));
    let consumed_end = consumed_end
        .unwrap_or_else(|| panic!("{backend} command boundary has no presented transport"));
    assert!(
        consumed_end <= published_end,
        "{backend} presented transport {consumed_end} is ahead of published transport {published_end}"
    );
    for command in 0..case.burst {
        queue.set_rate(if command.is_multiple_of(2) { 4.0 } else { 0.5 });
    }
    queue.set_rate(case.target_rate);
    samples.extend(capture_frames(&harness, case.observation_frames(), case.callback_frames).await);
    let events = recorder.snapshot();

    let precommand_start = command_frame.saturating_sub(TARGET_WINDOW_FRAMES);
    let precommand =
        &samples[precommand_start * usize::from(CHANNELS)..command_frame * usize::from(CHANNELS)];
    assert!(
        tone_is_dominant(precommand, case.initial_tone),
        "{backend} was not playing the initial tone before set_rate"
    );
    assert!(
        !tone_is_dominant(precommand, case.target_tone),
        "{backend} already contained the target tone before set_rate"
    );
    assert_response(backend, case, command_frame, &samples, &events);
}

#[kithara::test(
    tokio,
    multi_thread,
    serial,
    flash(false),
    timeout(Duration::from_secs(60)),
    hang_timeout_secs(5)
)]
#[case::signalsmith_minimum(StretchKind::Signalsmith, ElasticBackendConfig::default(), MINIMUM)]
#[case::signalsmith_product(StretchKind::Signalsmith, ElasticBackendConfig::default(), PRODUCT)]
#[case::signalsmith_extreme(StretchKind::Signalsmith, ElasticBackendConfig::default(), EXTREME)]
#[cfg_attr(
    not(all(target_os = "windows", target_env = "msvc")),
    case::bungee_minimum(StretchKind::Bungee, ElasticBackendConfig::default(), MINIMUM)
)]
#[cfg_attr(
    not(all(target_os = "windows", target_env = "msvc")),
    case::bungee_product(StretchKind::Bungee, ElasticBackendConfig::default(), PRODUCT)
)]
#[cfg_attr(
    not(all(target_os = "windows", target_env = "msvc")),
    case::bungee_extreme(StretchKind::Bungee, ElasticBackendConfig::default(), EXTREME)
)]
async fn live_rate_change_reaches_presented_pcm_within_response_budget(
    temp_dir: TestTempDir,
    #[case] backend: StretchKind,
    #[case] backends: ElasticBackendConfig,
    #[case] case: ResponseCase,
) {
    run_case(&temp_dir, backend, backends, case).await;
}
