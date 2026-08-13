#![cfg(not(target_arch = "wasm32"))]

use std::sync::mpsc::{SyncSender, sync_channel};

use kithara::{
    audio::{
        Audio, AudioConfig, AudioEffect, AudioWorkerHandle, PcmSession, StretchControls,
        StretchKind,
    },
    decode::PcmChunk,
    platform::{
        CancelToken,
        time::{self, Duration, Instant},
    },
    stream::Stream,
};
use kithara_integration_tests::{
    audio_artifact::write_audio_artifact,
    cochlea::{CochleaReport, assert_oracle_load_bearing, continuity_failures},
    kithara,
    memory_source::{MemStream, MemStreamConfig, MemorySource},
    offline::{OfflinePlayer, resource_from_reader},
    signal_pcm::signal::SignalFn,
    wav::create_wav,
};
use num_traits::ToPrimitive;
use serde::Serialize;

const SAMPLE_RATE: u32 = 44_100;
const CHANNELS: u16 = 2;
const BLOCK_FRAMES: usize = 512;
const SOURCE_SECONDS: usize = 6;
const WARMUP_BLOCKS: usize = 32;
const CAPTURE_BLOCKS: usize = 96;
const LOAD_INTERVAL_BLOCKS: usize = 8;
const LOAD_BURST: Duration = Duration::from_millis(18);
const MIN_CAPTURE_LOAD_BURSTS: usize = CAPTURE_BLOCKS / LOAD_INTERVAL_BLOCKS / 2;

struct QuietSine;

impl SignalFn for QuietSine {
    fn sample(&self, frame: usize, sample_rate: u32) -> i16 {
        let frame = frame.to_f64().expect("WAV frame fits f64");
        let phase = std::f64::consts::TAU * 440.0 * frame / f64::from(sample_rate);
        (phase.sin() * 16_000.0)
            .to_i16()
            .expect("quiet sine fits i16")
    }
}

struct BurstLoadEffect {
    blocks: usize,
    observed: SyncSender<()>,
}

impl BurstLoadEffect {
    fn new(observed: SyncSender<()>) -> Self {
        Self {
            blocks: 0,
            observed,
        }
    }
}

impl AudioEffect for BurstLoadEffect {
    fn flush(&mut self) -> Option<PcmChunk> {
        None
    }

    fn process(&mut self, chunk: PcmChunk) -> Option<PcmChunk> {
        self.blocks = self.blocks.saturating_add(1);
        if self.blocks.is_multiple_of(LOAD_INTERVAL_BLOCKS) {
            let _ = self.observed.try_send(());
            let deadline = Instant::now() + LOAD_BURST;
            while Instant::now() < deadline {
                std::hint::spin_loop();
            }
        }
        Some(chunk)
    }

    fn reset(&mut self) {
        self.blocks = 0;
    }
}

#[derive(Debug)]
struct RealtimeCapture {
    pcm: Vec<f32>,
    warmup_decode_errors: u64,
    warmup_underruns: u64,
    decode_errors: u64,
    underruns: u64,
    load_bursts: usize,
}

#[derive(Serialize)]
struct CaptureMetrics {
    warmup_decode_errors: u64,
    warmup_underruns: u64,
    decode_errors: u64,
    underruns: u64,
    load_bursts: usize,
}

#[derive(Debug, Serialize)]
struct SineFit {
    amplitude: f64,
    dc: f64,
    non_finite_samples: usize,
    residual_ratio: f64,
    stereo_mismatches: usize,
}

impl From<&RealtimeCapture> for CaptureMetrics {
    fn from(capture: &RealtimeCapture) -> Self {
        Self {
            warmup_decode_errors: capture.warmup_decode_errors,
            warmup_underruns: capture.warmup_underruns,
            decode_errors: capture.decode_errors,
            underruns: capture.underruns,
            load_bursts: capture.load_bursts,
        }
    }
}

#[derive(Serialize)]
struct PassthroughManifest<'a> {
    case: &'static str,
    sample_rate: u32,
    channels: u16,
    block_frames: usize,
    baseline: CaptureMetrics,
    unity: CaptureMetrics,
    unity_under_load: CaptureMetrics,
    baseline_source_fit: &'a SineFit,
    baseline_cochlea: &'a CochleaReport,
    unity_cochlea: &'a CochleaReport,
    unity_under_load_cochlea: &'a CochleaReport,
    failures: &'a [String],
}

fn measure_quiet_sine(samples: &[f32]) -> SineFit {
    let channels = usize::from(CHANNELS);
    let frames = samples.len() / channels;
    let mut non_finite_samples = 0;
    let mut stereo_mismatches = 0;
    let mut sum = 0.0;
    let mut sum_sin = 0.0;
    let mut sum_cos = 0.0;

    for frame in 0..frames {
        let left = samples[frame * channels];
        non_finite_samples += usize::from(!left.is_finite());
        stereo_mismatches += samples[frame * channels + 1..(frame + 1) * channels]
            .iter()
            .filter(|sample| sample.to_bits() != left.to_bits())
            .count();
        let phase = std::f64::consts::TAU * 440.0 * frame.to_f64().expect("capture frame fits f64")
            / f64::from(SAMPLE_RATE);
        let sample = f64::from(left);
        sum += sample;
        sum_sin += sample * phase.sin();
        sum_cos += sample * phase.cos();
    }

    let count = frames.to_f64().expect("capture length fits f64");
    let dc = sum / count;
    let sin_coefficient = 2.0 * sum_sin / count;
    let cos_coefficient = 2.0 * sum_cos / count;
    let amplitude = sin_coefficient.hypot(cos_coefficient);
    let mut signal_energy = 0.0;
    let mut residual_energy = 0.0;
    for frame in 0..frames {
        let phase = std::f64::consts::TAU * 440.0 * frame.to_f64().expect("capture frame fits f64")
            / f64::from(SAMPLE_RATE);
        let sample = f64::from(samples[frame * channels]);
        let centered = sample - dc;
        let fitted = sin_coefficient * phase.sin() + cos_coefficient * phase.cos();
        signal_energy += centered * centered;
        residual_energy += (centered - fitted) * (centered - fitted);
    }

    SineFit {
        amplitude,
        dc,
        non_finite_samples,
        residual_ratio: (residual_energy / signal_energy).sqrt(),
        stereo_mismatches,
    }
}

fn audio_config(
    worker: AudioWorkerHandle,
    stretch: bool,
    effects: Vec<Box<dyn AudioEffect>>,
) -> AudioConfig<MemStream> {
    let stream = MemStreamConfig {
        source: Some(MemorySource::new(create_wav(
            QuietSine,
            usize::try_from(SAMPLE_RATE).expect("sample rate fits usize") * SOURCE_SECONDS,
            SAMPLE_RATE,
            CHANNELS,
        ))),
        event_bus: None,
    };
    let stretch = stretch.then(|| {
        let controls = StretchControls::new(1.0);
        controls.set_backend(StretchKind::Signalsmith);
        controls.set_keylock(true);
        controls
    });

    AudioConfig::<MemStream>::for_stream(stream)
        .byte_pool(kithara::bufpool::BytePool::default())
        .pcm_pool(kithara::bufpool::PcmPool::default())
        .maybe_stretch(stretch)
        .worker(worker)
        .effects(effects)
        .hint("wav".to_owned())
        .build()
}

async fn wait_for_preload(audio: &Audio<Stream<MemStream>>) {
    let gate = audio
        .preload_gate()
        .expect("worker-backed audio exposes a preload gate");
    time::timeout(
        Duration::from_secs(5),
        gate.wait_for_epoch(audio.preload_epoch()),
    )
    .await
    .expect("audio preload gate must open");
}

async fn render_passthrough(stretch: bool, with_load: bool) -> RealtimeCapture {
    let (load_observed_tx, load_observed_rx) =
        sync_channel((WARMUP_BLOCKS + CAPTURE_BLOCKS) / LOAD_INTERVAL_BLOCKS + 2);
    let worker = AudioWorkerHandle::with_cancel(CancelToken::never());
    let mut target_audio =
        Audio::<Stream<MemStream>>::new(audio_config(worker.clone(), stretch, Vec::new()))
            .await
            .expect("target audio construction");
    wait_for_preload(&target_audio).await;
    target_audio.preload().expect("target preload");

    let mut load_audio = if with_load {
        let mut audio = Audio::<Stream<MemStream>>::new(audio_config(
            worker,
            false,
            vec![Box::new(BurstLoadEffect::new(load_observed_tx))],
        ))
        .await
        .expect("load audio construction");
        wait_for_preload(&audio).await;
        audio.preload().expect("load preload");
        Some(audio)
    } else {
        None
    };

    let mut target = OfflinePlayer::new(SAMPLE_RATE);
    target.set_fade_duration(0.0);
    target.load_and_fadein(resource_from_reader(target_audio), "passthrough-target");
    let mut load = load_audio.take().map(|audio| {
        let mut player = OfflinePlayer::new(SAMPLE_RATE);
        player.set_fade_duration(0.0);
        player.load_and_fadein(resource_from_reader(audio), "shared-worker-load");
        player
    });
    let block_period = Duration::from_secs_f64(
        f64::from(u32::try_from(BLOCK_FRAMES).expect("block size fits u32"))
            / f64::from(SAMPLE_RATE),
    );

    let metrics_before_warmup = target.metrics();
    for _ in 0..WARMUP_BLOCKS {
        let started = Instant::now();
        if let Some(player) = load.as_mut() {
            let _ = player.render(BLOCK_FRAMES);
        }
        let _ = target.render(BLOCK_FRAMES);
        time::sleep(block_period.saturating_sub(started.elapsed())).await;
    }
    while load_observed_rx.try_recv().is_ok() {}

    let metrics_before_capture = target.metrics();
    let warmup_decode_errors = metrics_before_capture
        .decode_errors()
        .saturating_sub(metrics_before_warmup.decode_errors());
    let warmup_underruns = metrics_before_capture
        .underruns()
        .saturating_sub(metrics_before_warmup.underruns());
    let mut pcm = Vec::with_capacity(CAPTURE_BLOCKS * BLOCK_FRAMES * usize::from(CHANNELS));
    for _ in 0..CAPTURE_BLOCKS {
        let started = Instant::now();
        if let Some(player) = load.as_mut() {
            let _ = player.render(BLOCK_FRAMES);
        }
        pcm.extend(target.render(BLOCK_FRAMES));
        time::sleep(block_period.saturating_sub(started.elapsed())).await;
    }
    let metrics_after_capture = target.metrics();

    RealtimeCapture {
        pcm,
        warmup_decode_errors,
        warmup_underruns,
        decode_errors: metrics_after_capture
            .decode_errors()
            .saturating_sub(metrics_before_capture.decode_errors()),
        underruns: metrics_after_capture
            .underruns()
            .saturating_sub(metrics_before_capture.underruns()),
        load_bursts: load_observed_rx.try_iter().count(),
    }
}

fn first_sample_mismatch(candidate: &[f32], control: &[f32]) -> Option<usize> {
    candidate
        .iter()
        .zip(control)
        .position(|(candidate, control)| candidate.to_bits() != control.to_bits())
        .or_else(|| (candidate.len() != control.len()).then(|| candidate.len().min(control.len())))
}

#[kithara::test(
    tokio,
    flash(false),
    serial,
    timeout(Duration::from_secs(30)),
    env(KITHARA_HANG_TIMEOUT_SECS = "5")
)]
async fn no_sync_unity_playback_is_bit_exact_and_cochlea_clean_under_load() {
    let channels = usize::from(CHANNELS);
    let baseline = render_passthrough(false, false).await;
    let baseline_report = CochleaReport::measure(&baseline.pcm, CHANNELS, SAMPLE_RATE);
    let baseline_source_fit = measure_quiet_sine(&baseline.pcm);
    let unity = render_passthrough(true, false).await;
    let unity_report = CochleaReport::measure(&unity.pcm, CHANNELS, SAMPLE_RATE);
    let loaded = render_passthrough(true, true).await;
    let loaded_report = CochleaReport::measure(&loaded.pcm, CHANNELS, SAMPLE_RATE);
    let mut failures = Vec::new();
    if !baseline.pcm.iter().any(|sample| sample.abs() > 0.25) {
        failures.push("effect-free control contains no audible PCM".to_owned());
    }
    if baseline_source_fit.non_finite_samples != 0
        || baseline_source_fit.stereo_mismatches != 0
        || !(0.47..=0.50).contains(&baseline_source_fit.amplitude)
        || baseline_source_fit.dc.abs() > 0.002
        || baseline_source_fit.residual_ratio > 0.01
    {
        failures.push(format!(
            "effect-free control does not match the independent 440 Hz source oracle: {baseline_source_fit:?}",
        ));
    }
    if baseline_report.silent_segments != 0 {
        failures.push(format!(
            "effect-free: {} silent Cochlea segment(s)",
            baseline_report.silent_segments,
        ));
    }
    if baseline_report.onset_count() != 0 {
        failures.push(format!(
            "effect-free: {} unexpected Cochlea onset(s)",
            baseline_report.onset_count(),
        ));
    }
    if baseline_report.clipped_samples != 0 || baseline_report.true_peak_over_0dbtp {
        failures.push(format!(
            "effect-free: clipping evidence samples={}, true_peak_over_0dbtp={}",
            baseline_report.clipped_samples, baseline_report.true_peak_over_0dbtp,
        ));
    }

    for (label, capture, report) in [
        ("effect-free", &baseline, &baseline_report),
        ("unity", &unity, &unity_report),
        ("unity+load", &loaded, &loaded_report),
    ] {
        let non_finite = capture
            .pcm
            .iter()
            .filter(|sample| !sample.is_finite())
            .count();
        if non_finite != 0 {
            failures.push(format!("{label}: {non_finite} non-finite PCM sample(s)"));
        }
        if capture.warmup_decode_errors != 0 || capture.decode_errors != 0 {
            failures.push(format!(
                "{label}: decode errors warmup={}, capture={}",
                capture.warmup_decode_errors, capture.decode_errors,
            ));
        }
        if capture.warmup_underruns != 0 || capture.underruns != 0 {
            failures.push(format!(
                "{label}: underruns warmup={}, capture={}",
                capture.warmup_underruns, capture.underruns,
            ));
        }
        if label != "effect-free" {
            failures.extend(continuity_failures(label, report, &baseline_report));
            if let Some(sample) = first_sample_mismatch(&capture.pcm, &baseline.pcm) {
                failures.push(format!(
                    "{label}: PCM differs at sample {sample} (frame {})",
                    sample / channels,
                ));
            }
        }
    }
    if loaded.load_bursts < MIN_CAPTURE_LOAD_BURSTS {
        failures.push(format!(
            "unity+load: only {} bounded shared-worker bursts ran during capture; expected at least {MIN_CAPTURE_LOAD_BURSTS}",
            loaded.load_bursts,
        ));
    }

    let manifest = PassthroughManifest {
        case: "no-sync-unity-passthrough",
        sample_rate: SAMPLE_RATE,
        channels: CHANNELS,
        block_frames: BLOCK_FRAMES,
        baseline: CaptureMetrics::from(&baseline),
        unity: CaptureMetrics::from(&unity),
        unity_under_load: CaptureMetrics::from(&loaded),
        baseline_source_fit: &baseline_source_fit,
        baseline_cochlea: &baseline_report,
        unity_cochlea: &unity_report,
        unity_under_load_cochlea: &loaded_report,
        failures: &failures,
    };
    write_audio_artifact(
        "no-sync-unity-passthrough",
        SAMPLE_RATE,
        CHANNELS,
        &[
            ("effect-free-control", &baseline.pcm),
            ("unity", &unity.pcm),
            ("unity-under-load", &loaded.pcm),
        ],
        &manifest,
    )
    .expect("optional no-SYNC audio artifact write");

    assert_oracle_load_bearing(&baseline.pcm, CHANNELS, SAMPLE_RATE, BLOCK_FRAMES);
    assert!(
        failures.is_empty(),
        "no-SYNC playback was not transparent: {}\nbaseline={baseline_report:?}\nunity={unity_report:?}\nunity+load={loaded_report:?}",
        failures.join("; "),
    );
}
