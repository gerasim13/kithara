#![cfg(not(target_arch = "wasm32"))]

use std::sync::mpsc::{SyncSender, sync_channel};

use kithara::{
    audio::{
        Audio, AudioConfig, AudioEffect, AudioWorkerHandle, ChunkOutcome, DecodeResult, PcmRead,
        PcmSession, ReadOutcome, StretchControls, StretchKind, TempoSlot,
    },
    decode::{PcmChunk, PcmMeta},
    events::{AudioEvent, Event, SeekEpoch, SeekLifecycleStage},
    platform::{
        CancelToken,
        time::{self, Duration, Instant},
    },
    stream::Stream,
};
use kithara_integration_tests::{
    cochlea::{CochleaReport, assert_oracle_load_bearing, continuity_failures},
    create_test_wav, kithara,
    memory_source::{MemStream, MemStreamConfig, MemorySource},
    offline::{OfflinePlayer, resource_from_reader},
    reads::blocking_audio,
    sync_artifact::{
        ArtifactAudio, ArtifactFrame, ArtifactSource, SyncArtifactMetadata, write_sync_artifact,
    },
};
use num_traits::ToPrimitive;

const SOURCE_RATE: u32 = 44_100;
const CHANNELS: u16 = 2;
const PRESENTATION_FRAMES: usize = 512;
const REALTIME_SOURCE_SECONDS: usize = 6;
const WARMUP_BLOCKS: usize = 32;
const CAPTURE_BLOCKS: usize = 96;
const LOAD_INTERVAL_BLOCKS: usize = 8;
const LOAD_BURST: Duration = Duration::from_millis(18);

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

    fn held_source_frames(&self) -> u64 {
        0
    }

    fn process(&mut self, chunk: PcmChunk) -> DecodeResult<Option<PcmChunk>> {
        self.blocks = self.blocks.saturating_add(1);
        if self.blocks.is_multiple_of(LOAD_INTERVAL_BLOCKS) {
            let _ = self.observed.try_send(());
            let deadline = Instant::now() + LOAD_BURST;
            while Instant::now() < deadline {
                std::hint::spin_loop();
            }
        }
        Ok(Some(chunk))
    }

    fn reset(&mut self) {
        self.blocks = 0;
    }
}

#[derive(Debug)]
struct RealtimeCapture {
    pcm: Vec<f32>,
    warmup_underruns: u64,
    underruns: u64,
    load_burst_observed: bool,
}

fn exact_wav_config(speed: Option<f32>) -> AudioConfig<MemStream> {
    let stretch = speed.map(|speed| {
        let controls = StretchControls::new(speed);
        controls.set_backend(StretchKind::Signalsmith);
        controls.set_keylock(true);
        controls
    });
    let stream = MemStreamConfig {
        source: Some(MemorySource::new(create_test_wav(
            usize::try_from(SOURCE_RATE).expect("sample rate fits usize") * 2,
            SOURCE_RATE,
            2,
        ))),
        event_bus: None,
    };

    AudioConfig::<MemStream>::for_stream(stream)
        .byte_pool(kithara::bufpool::BytePool::default())
        .pcm_pool(kithara::bufpool::PcmPool::default())
        .maybe_tempo(stretch.map(TempoSlot::from))
        .pcm_buffer_chunks(1)
        .hint("wav".to_owned())
        .build()
}

fn realtime_wav_config(
    worker: AudioWorkerHandle,
    tempo: bool,
    effects: Vec<Box<dyn AudioEffect>>,
) -> AudioConfig<MemStream> {
    let stream = MemStreamConfig {
        source: Some(MemorySource::new(create_test_wav(
            usize::try_from(SOURCE_RATE).expect("sample rate fits usize") * REALTIME_SOURCE_SECONDS,
            SOURCE_RATE,
            CHANNELS,
        ))),
        event_bus: None,
    };
    let tempo = tempo.then(|| {
        let controls = StretchControls::new(1.0);
        controls.set_backend(StretchKind::Signalsmith);
        controls.set_keylock(true);
        TempoSlot::from(controls)
    });

    AudioConfig::<MemStream>::for_stream(stream)
        .byte_pool(kithara::bufpool::BytePool::default())
        .pcm_pool(kithara::bufpool::PcmPool::default())
        .maybe_tempo(tempo)
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

async fn render_realtime_passthrough(tempo: bool, with_load: bool) -> RealtimeCapture {
    let (load_observed_tx, load_observed_rx) = sync_channel(1);
    let worker = AudioWorkerHandle::with_cancel(CancelToken::never());
    let mut target_audio =
        Audio::<Stream<MemStream>>::new(realtime_wav_config(worker.clone(), tempo, Vec::new()))
            .await
            .expect("unity passthrough audio construction");
    wait_for_preload(&target_audio).await;
    target_audio.preload().expect("target preload");

    let mut load_audio = if with_load {
        let mut audio = Audio::<Stream<MemStream>>::new(realtime_wav_config(
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

    let mut target = OfflinePlayer::new(SOURCE_RATE);
    target.set_fade_duration(0.0);
    target.load_and_fadein(resource_from_reader(target_audio), "passthrough-target");
    let mut load = load_audio.take().map(|audio| {
        let mut player = OfflinePlayer::new(SOURCE_RATE);
        player.set_fade_duration(0.0);
        player.load_and_fadein(resource_from_reader(audio), "shared-worker-load");
        player
    });
    let block_period = Duration::from_secs_f64(
        PRESENTATION_FRAMES
            .to_f64()
            .expect("presentation block fits f64")
            / f64::from(SOURCE_RATE),
    );

    let underruns_before_warmup = target.metrics().underruns();
    for _ in 0..WARMUP_BLOCKS {
        let started = Instant::now();
        if let Some(player) = load.as_mut() {
            let _ = player.render(PRESENTATION_FRAMES);
        }
        let _ = target.render(PRESENTATION_FRAMES);
        time::sleep(block_period.saturating_sub(started.elapsed())).await;
    }
    while load_observed_rx.try_recv().is_ok() {}

    let underruns_before = target.metrics().underruns();
    let warmup_underruns = underruns_before.saturating_sub(underruns_before_warmup);
    let mut pcm = Vec::with_capacity(CAPTURE_BLOCKS * PRESENTATION_FRAMES * usize::from(CHANNELS));
    for _ in 0..CAPTURE_BLOCKS {
        let started = Instant::now();
        if let Some(player) = load.as_mut() {
            let _ = player.render(PRESENTATION_FRAMES);
        }
        pcm.extend(target.render(PRESENTATION_FRAMES));
        time::sleep(block_period.saturating_sub(started.elapsed())).await;
    }
    let underruns = target
        .metrics()
        .underruns()
        .saturating_sub(underruns_before);

    RealtimeCapture {
        pcm,
        warmup_underruns,
        underruns,
        load_burst_observed: load_observed_rx.try_recv().is_ok(),
    }
}

fn first_sample_mismatch(candidate: &[f32], control: &[f32]) -> Option<usize> {
    candidate
        .iter()
        .zip(control)
        .position(|(candidate, control)| candidate.to_bits() != control.to_bits())
        .or_else(|| (candidate.len() != control.len()).then(|| candidate.len().min(control.len())))
}

async fn collect_exact_pcm(
    audio: &mut Audio<Stream<MemStream>>,
    budget: Duration,
) -> Vec<(PcmMeta, Vec<u32>)> {
    let deadline = Instant::now() + budget;
    let mut chunks = Vec::new();
    while Instant::now() < deadline {
        match audio.next_chunk() {
            Ok(ChunkOutcome::Chunk(chunk)) => chunks.push((
                chunk.meta,
                chunk
                    .samples
                    .iter()
                    .map(|sample| sample.to_bits())
                    .collect(),
            )),
            Ok(ChunkOutcome::Pending { .. }) => {
                time::sleep(Duration::from_millis(1)).await;
            }
            Ok(ChunkOutcome::Eof { .. }) => return chunks,
            Err(error) => panic!("decode failed while collecting PCM: {error}"),
        }
    }
    panic!("timed out collecting exact PCM");
}

fn wav_stream(samples: usize) -> AudioConfig<MemStream> {
    let wav = create_test_wav(samples, 44_100, 2);
    let source = MemorySource::new(wav);
    let stream = MemStreamConfig {
        source: Some(source),
        event_bus: None,
    };
    AudioConfig::<MemStream>::for_stream(stream)
        .byte_pool(kithara::bufpool::BytePool::default())
        .pcm_pool(kithara::bufpool::PcmPool::default())
        .hint("wav".to_string())
        .build()
}

async fn wait_for_frames<S>(mut audio: Audio<S>, budget: Duration) -> (Audio<S>, usize)
where
    Audio<S>: Send + 'static,
{
    let mut buf = [0.0f32; 256];
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        let (next_audio, (next_buf, outcome)) = blocking_audio(audio, move |audio| {
            let outcome = audio.read(&mut buf);
            (buf, outcome)
        })
        .await;
        audio = next_audio;
        buf = next_buf;
        match outcome {
            Ok(ReadOutcome::Frames { count, .. }) => return (audio, count.get()),
            Ok(ReadOutcome::Eof { .. }) => return (audio, 0),
            Ok(ReadOutcome::Pending { .. }) => {
                time::sleep(Duration::from_millis(20)).await;
            }
            Err(error) => panic!("decode error while waiting for frames: {error}"),
        }
    }
    panic!("timed out waiting for ReadOutcome::Frames");
}

async fn drain_to_eof<S>(mut audio: Audio<S>, budget: Duration) -> (Audio<S>, usize)
where
    Audio<S>: Send + 'static,
{
    let mut buf = [0.0f32; 4096];
    let mut total = 0usize;
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        let (next_audio, (next_buf, outcome)) = blocking_audio(audio, move |audio| {
            let outcome = audio.read(&mut buf);
            (buf, outcome)
        })
        .await;
        audio = next_audio;
        buf = next_buf;
        match outcome {
            Ok(ReadOutcome::Frames { count, .. }) => total += count.get(),
            Ok(ReadOutcome::Eof { .. }) => return (audio, total),
            Ok(ReadOutcome::Pending { .. }) => {
                time::sleep(Duration::from_millis(10)).await;
            }
            Err(error) => panic!("decode error while draining: {error}"),
        }
    }
    panic!("timed out before reaching Eof; collected {total} frames");
}

#[kithara::test(tokio, timeout(Duration::from_secs(10)))]
async fn basic_decode_to_eof() {
    let config = wav_stream(8_000);
    let audio = Audio::<Stream<MemStream>>::new(config)
        .await
        .expect("audio construction");

    let (_audio, frames) = drain_to_eof(audio, Duration::from_secs(5)).await;
    assert!(
        frames >= 8_000,
        "expected at least the input frame count, got {frames}"
    );
}

#[kithara::test(tokio, timeout(Duration::from_secs(20)))]
async fn unity_stretch_chunks_are_bit_identical_before_rt_playback() {
    let baseline = {
        let mut audio = Audio::<Stream<MemStream>>::new(exact_wav_config(None))
            .await
            .expect("baseline audio construction");
        collect_exact_pcm(&mut audio, Duration::from_secs(5)).await
    };
    let unity = {
        let mut audio = Audio::<Stream<MemStream>>::new(exact_wav_config(Some(1.0)))
            .await
            .expect("unity audio construction");
        collect_exact_pcm(&mut audio, Duration::from_secs(5)).await
    };

    assert_eq!(unity, baseline);
}

#[kithara::test(
    tokio,
    flash(false),
    serial,
    timeout(Duration::from_secs(30)),
    env(KITHARA_HANG_TIMEOUT_SECS = "5")
)]
async fn unity_stretch_pipeline_is_bit_identical_to_the_effect_free_path() {
    let channels = usize::from(CHANNELS);
    let baseline = render_realtime_passthrough(false, false).await;
    let baseline_metric = CochleaReport::measure(&baseline.pcm, CHANNELS, SOURCE_RATE);

    assert!(
        baseline.pcm.iter().any(|sample| sample.abs() > 0.5),
        "effect-free baseline must contain audible PCM"
    );
    assert_oracle_load_bearing(&baseline.pcm, CHANNELS, SOURCE_RATE, PRESENTATION_FRAMES);

    let unity = render_realtime_passthrough(true, false).await;
    let unity_metric = CochleaReport::measure(&unity.pcm, CHANNELS, SOURCE_RATE);
    let loaded = render_realtime_passthrough(true, true).await;
    let loaded_metric = CochleaReport::measure(&loaded.pcm, CHANNELS, SOURCE_RATE);
    let mut failures = Vec::new();
    if baseline.warmup_underruns != 0 {
        failures.push(format!(
            "effect-free baseline warmup: {} real-time underrun(s)",
            baseline.warmup_underruns
        ));
    }
    if baseline.underruns != 0 {
        failures.push(format!(
            "effect-free baseline: {} real-time underrun(s)",
            baseline.underruns
        ));
    }
    if baseline_metric.silent_segments != 0 {
        failures.push(format!(
            "effect-free baseline: {} Cochlea silent segment(s)",
            baseline_metric.silent_segments
        ));
    }
    if baseline_metric.onset_count() != 0 {
        failures.push(format!(
            "effect-free baseline: {} unexpected Cochlea onset(s)",
            baseline_metric.onset_count()
        ));
    }
    if baseline_metric.clipped_samples != 0 {
        failures.push(format!(
            "effect-free baseline: {} clipped sample(s)",
            baseline_metric.clipped_samples
        ));
    }

    for (label, capture, metric) in [
        ("unity", &unity, &unity_metric),
        ("unity+load", &loaded, &loaded_metric),
    ] {
        if capture.warmup_underruns != 0 {
            failures.push(format!(
                "{label} warmup: {} real-time underrun(s)",
                capture.warmup_underruns
            ));
        }
        if capture.underruns != 0 {
            failures.push(format!(
                "{label}: {} real-time underrun(s)",
                capture.underruns
            ));
        }
        failures.extend(continuity_failures(label, metric, &baseline_metric));
        if let Some(sample) = first_sample_mismatch(&capture.pcm, &baseline.pcm) {
            failures.push(format!(
                "{label}: PCM diverged from effect-free baseline at sample {sample} (frame {})",
                sample / channels
            ));
        }
    }
    if !loaded.load_burst_observed {
        failures.push("unity+load: synthetic worker burst did not run".to_owned());
    }

    let mut metadata = SyncArtifactMetadata::new(
        "unity-passthrough-real-callback-cadence",
        SOURCE_RATE,
        CHANNELS,
        PRESENTATION_FRAMES,
    );
    metadata.add_source(ArtifactSource::new("deck-a", "deterministic-wav"));
    metadata.set_operation("free unity passthrough with and without shared-worker load");
    metadata.add_frame(ArtifactFrame::new(
        u64::try_from(WARMUP_BLOCKS * PRESENTATION_FRAMES).expect("warmup frame fits u64"),
        "capture-start",
    ));
    metadata.add_threshold("extra_cochlea_failures", 0.0);
    metadata.add_failures(failures.clone());
    write_sync_artifact(
        &metadata,
        &[
            ArtifactAudio::new("effect-free-control", &baseline.pcm),
            ArtifactAudio::new("unity", &unity.pcm),
            ArtifactAudio::new("unity-under-load", &loaded.pcm),
        ],
    )
    .expect("optional unity passthrough artifact writes");

    assert!(
        failures.is_empty(),
        "unity passthrough distorted at real callback cadence: {}\nbaseline={baseline_metric:?}\nunity={unity_metric:?}\nunity+load={loaded_metric:?}",
        failures.join("; ")
    );
}

#[kithara::test(tokio, timeout(Duration::from_secs(10)))]
async fn seek_during_active_decode_completes_without_hang() {
    let config = wav_stream(44_100 * 3);
    let audio = Audio::<Stream<MemStream>>::new(config)
        .await
        .expect("audio construction");
    let mut events = audio.event_bus().subscribe();

    let (audio, _initial_frames) = wait_for_frames(audio, Duration::from_secs(2)).await;
    let (mut audio, seek_result) =
        blocking_audio(audio, |audio| audio.seek(Duration::from_secs_f64(1.5))).await;
    seek_result.expect("seek");

    let mut observed_epoch: Option<SeekEpoch> = None;
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if let Ok(Ok(Event::Audio(AudioEvent::SeekLifecycle {
            stage: SeekLifecycleStage::SeekRequest,
            seek_epoch,
            ..
        }))) = time::timeout(remaining, events.recv())
            .await
            .map(|r| r.map(|env| env.event))
        {
            observed_epoch = Some(seek_epoch);
            break;
        }
    }
    let expected_epoch = observed_epoch.expect("SeekLifecycle::SeekRequest event");

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut saw_complete = false;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match time::timeout(remaining, events.recv())
            .await
            .map(|r| r.map(|env| env.event))
        {
            Ok(Ok(Event::Audio(AudioEvent::SeekComplete { seek_epoch, .. })))
                if seek_epoch == expected_epoch =>
            {
                saw_complete = true;
                break;
            }
            Ok(_) => {
                let (next_audio, _frames) =
                    wait_for_frames(audio, Duration::from_millis(150)).await;
                audio = next_audio;
            }
            Err(_) => break,
        }
    }
    assert!(saw_complete, "SeekComplete must arrive after seek");

    let (_audio, frames_after) = wait_for_frames(audio, Duration::from_secs(2)).await;
    assert!(
        frames_after > 0,
        "audio must keep producing frames after seek"
    );
}

#[kithara::test(tokio, timeout(Duration::from_secs(15)))]
async fn rapid_seeks_via_timeline_all_complete() {
    const SEEK_COUNT: usize = 6;

    let config = wav_stream(44_100 * 4);
    let mut audio = Audio::<Stream<MemStream>>::new(config)
        .await
        .expect("audio construction");
    let mut events = audio.event_bus().subscribe();

    // Keep the settle reads inline so the flash rewriter retargets these
    // sleeps onto the virtual clock. `Audio::seek()` publishes SeekRequest
    // synchronously; SeekComplete / PlaybackProgress still require reads to
    // commit post-seek output.

    // Prime: read until the first decoded frames arrive.
    {
        let mut buf = [0.0f32; 256];
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            let (next_audio, (next_buf, outcome)) = blocking_audio(audio, move |audio| {
                let outcome = audio.read(&mut buf);
                (buf, outcome)
            })
            .await;
            audio = next_audio;
            buf = next_buf;
            match outcome {
                Ok(ReadOutcome::Frames { .. }) | Ok(ReadOutcome::Eof { .. }) => break,
                Ok(ReadOutcome::Pending { .. }) => {
                    time::sleep(Duration::from_millis(20)).await;
                }
                Err(error) => panic!("decode error while priming: {error}"),
            }
        }
    }

    let mut expected_epochs = Vec::with_capacity(SEEK_COUNT);
    for i in 0..SEEK_COUNT {
        let target = Duration::from_millis(200 + (i as u64) * 250);
        let (next_audio, seek_result) =
            blocking_audio(audio, move |audio| audio.seek(target)).await;
        audio = next_audio;
        seek_result.expect("seek");

        let deadline = Instant::now() + Duration::from_secs(1);
        let mut captured = None;
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if let Ok(Ok(Event::Audio(AudioEvent::SeekLifecycle {
                stage: SeekLifecycleStage::SeekRequest,
                seek_epoch,
                ..
            }))) = time::timeout(remaining, events.recv())
                .await
                .map(|r| r.map(|env| env.event))
            {
                captured = Some(seek_epoch);
                break;
            }
        }
        expected_epochs.push(captured.expect("seek epoch from SeekRequest"));

        // Settle on the virtual clock: read post-seek frames so the consumer
        // commits this seek and the worker advances before the next `seek()`.
        let mut buf = [0.0f32; 256];
        let deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < deadline {
            let (next_audio, (next_buf, outcome)) = blocking_audio(audio, move |audio| {
                let outcome = audio.read(&mut buf);
                (buf, outcome)
            })
            .await;
            audio = next_audio;
            buf = next_buf;
            match outcome {
                Ok(ReadOutcome::Frames { .. }) | Ok(ReadOutcome::Eof { .. }) => break,
                Ok(ReadOutcome::Pending { .. }) => {
                    time::sleep(Duration::from_millis(20)).await;
                }
                Err(error) => panic!("decode error while settling seek: {error}"),
            }
        }
    }

    let highest_expected = *expected_epochs
        .iter()
        .max()
        .expect("at least one seek epoch");

    let mut buf = [0.0f32; 256];
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut last_complete: Option<SeekEpoch> = None;
    while Instant::now() < deadline {
        // Read each tick so the consumer keeps committing post-seek output and
        // emitting `SeekComplete` for the highest requested epoch. The
        // ownership roundtrip keeps a possible read park off the runtime;
        // `events.recv()` then yields on the virtual clock for the next chunk.
        let (next_audio, (next_buf, outcome)) = blocking_audio(audio, move |audio| {
            let outcome = audio.read(&mut buf);
            (buf, outcome)
        })
        .await;
        audio = next_audio;
        buf = next_buf;
        match outcome {
            Ok(ReadOutcome::Frames { .. })
            | Ok(ReadOutcome::Eof { .. })
            | Ok(ReadOutcome::Pending { .. }) => {}
            Err(error) => panic!("decode error while draining seek completions: {error}"),
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        match time::timeout(remaining, events.recv())
            .await
            .map(|r| r.map(|env| env.event))
        {
            Ok(Ok(Event::Audio(AudioEvent::SeekComplete { seek_epoch, .. }))) => {
                last_complete = Some(seek_epoch);
                if seek_epoch >= highest_expected {
                    break;
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    assert_eq!(
        last_complete,
        Some(highest_expected),
        "last observed SeekComplete must match the highest requested epoch"
    );
}

#[kithara::test(tokio, timeout(Duration::from_secs(10)))]
async fn truncated_wav_surfaces_decode_error_or_eof() {
    let mut wav = create_test_wav(44_100, 44_100, 2);
    wav.truncate(wav.len() / 4);
    let source = MemorySource::new(wav);
    let config = AudioConfig::<MemStream>::for_stream(MemStreamConfig {
        source: Some(source),
        event_bus: None,
    })
    .byte_pool(kithara::bufpool::BytePool::default())
    .pcm_pool(kithara::bufpool::PcmPool::default())
    .hint("wav".to_string())
    .build();

    let audio = Audio::<Stream<MemStream>>::new(config)
        .await
        .expect("audio construction");

    let (_audio, saw_terminal) = blocking_audio(audio, |audio| {
        let mut buf = [0.0f32; 4096];
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match audio.read(&mut buf) {
                Ok(ReadOutcome::Eof { .. }) | Err(_) => return true,
                Ok(ReadOutcome::Frames { .. }) | Ok(ReadOutcome::Pending { .. }) => {}
            }
        }
        false
    })
    .await;
    assert!(
        saw_terminal,
        "truncated WAV must surface either Eof or DecodeError"
    );
}
