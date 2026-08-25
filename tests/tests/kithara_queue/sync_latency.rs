#![cfg(not(target_arch = "wasm32"))]

use std::{
    num::NonZeroU32,
    sync::atomic::{AtomicU64, Ordering},
};

use kithara::{
    audio::{
        AudioWorkerHandle, Beat, BeatGrid, BeatMap, BeatMapSnapshot, MapPoint, MapPosition,
        MapQuery, SessionFrame, StretchControls, StretchKind, analysis::TrackAnalysis,
    },
    bufpool::{BytePool, PcmPool},
    events::{AudioEvent, DecoderEvent, Event, EventReceiver},
    platform::{
        sync::{Arc, Mutex},
        time::Duration,
        tokio::{sync::broadcast::error::TryRecvError, task},
    },
    play::{Cmd, PlayerConfig, PlayerImpl, Reply, SessionDispatcher, Tempo, apply_mix},
    queue::{Queue, QueueConfig, TrackSource},
};
use kithara_integration_tests::{
    RhythmicTrack, SignalFormat, SignalSpec, SignalSpecLength, SweepMode, TestServerHelper,
    cochlea::{CochleaReport, assert_oracle_load_bearing, continuity_failures},
    kithara,
    ring::{ManualRingConfig, ManualRingSession},
    sync_artifact::{
        ArtifactAudio, ArtifactFrame, ArtifactSource, SyncArtifactMetadata, write_sync_artifact,
    },
    sync_control::{SyncDeckControl, SyncQuantum},
    sync_fixture::{SyncFixtureResources, analysis_map},
};
use num_traits::ToPrimitive;
use reqwest::{Client, StatusCode, header::RANGE};

const CHANNELS: u16 = 2;
const CHIRP_PEAK: f32 = 0.35;
const LOAD_PULL_LIMIT: usize = 16_000;
const PULSE_PEAK: f32 = 0.55;
const RHYTHMIC_MIX_PEAK: f32 = 0.6;
const SAMPLE_RATE: u32 = 48_000;
const SESSION_BPM: f64 = 120.0;
const TRACK_FRAMES: usize = 48_000 * 24;
const TWENTY_MS_FRAMES: usize = 960;

/// Tagged source phases for the serialized manual-ring command protocol.
/// This fixture does not model within-callback arrival or pending depth.
#[derive(Clone, Copy, Debug)]
enum TaggedSignalPhase {
    Early,
    Middle,
    Late,
}

impl TaggedSignalPhase {
    const ALL: [Self; 3] = [Self::Early, Self::Middle, Self::Late];

    const fn label(self) -> &'static str {
        match self {
            Self::Early => "early",
            Self::Middle => "middle",
            Self::Late => "late",
        }
    }

    const fn warm_blocks(self) -> usize {
        match self {
            Self::Early => 47,
            Self::Middle => 94,
            Self::Late => 140,
        }
    }
}

pub(super) struct SyncFixture {
    _server: TestServerHelper,
    chirp: SignalAsset,
    pulse: SignalAsset,
    resources: SyncFixtureResources,
}

pub(super) struct SignalAsset {
    gain: f32,
    source: TrackSource,
}

impl SignalAsset {
    pub(super) fn uri(&self) -> &str {
        self.source
            .uri()
            .expect("prepared signal always uses a UTF-8 URL")
    }
}

impl SyncFixture {
    pub(super) async fn new(resources: SyncFixtureResources) -> Self {
        let server = TestServerHelper::new().await;
        let spec = SignalSpec {
            format: SignalFormat::Wav,
            length: SignalSpecLength::Frames(TRACK_FRAMES),
            channels: CHANNELS,
            sample_rate: SAMPLE_RATE,
            bit_rate: None,
        };
        let chirp = server.sweep(&spec, 220.0, 1_180.0, SweepMode::Linear).await;
        let pulse = server
            .rhythmic_mix(&spec, &[RhythmicTrack::new(SESSION_BPM, 880.0)])
            .await;
        Self {
            _server: server,
            chirp: SignalAsset {
                gain: CHIRP_PEAK,
                source: chirp.to_string().into(),
            },
            pulse: SignalAsset {
                gain: PULSE_PEAK / RHYTHMIC_MIX_PEAK,
                source: pulse.to_string().into(),
            },
            resources,
        }
    }

    pub(super) const fn chirp(&self) -> &SignalAsset {
        &self.chirp
    }

    pub(super) const fn pulse(&self) -> &SignalAsset {
        &self.pulse
    }

    pub(super) const fn resources(&self) -> &SyncFixtureResources {
        &self.resources
    }
}

pub(super) fn sync_fixture_resources(case: &str) -> SyncFixtureResources {
    SyncFixtureResources::new(case, BytePool::default(), PcmPool::default())
        .expect("initialize sync fixture resources")
}

pub(super) struct RingDeck {
    block_frames: usize,
    events: Mutex<EventReceiver>,
    failures: Mutex<Vec<String>>,
    player: Arc<PlayerImpl>,
    queue: Arc<Queue>,
    resources: SyncFixtureResources,
    session: Arc<ManualRingSession>,
    underruns: AtomicU64,
}

impl RingDeck {
    pub(super) async fn load(
        resources: &SyncFixtureResources,
        signal: &SignalAsset,
        block_frames: usize,
    ) -> Self {
        let rate = NonZeroU32::new(SAMPLE_RATE).expect("fixture sample rate is non-zero");
        let block_u32 = u32::try_from(block_frames).expect("fixture block size fits u32");
        let session = Arc::new(
            ManualRingSession::start(ManualRingConfig::new(rate, block_u32, 4))
                .expect("manual ring session starts"),
        );
        let dispatcher = Arc::clone(&session) as Arc<dyn SessionDispatcher>;
        let controls = StretchControls::new(1.0);
        controls.set_backend(StretchKind::Signalsmith);
        controls.set_keylock(true);
        let player = Arc::new(PlayerImpl::new(
            PlayerConfig::builder()
                .byte_pool(resources.byte_pool().clone())
                .pcm_pool(resources.pcm_pool().clone())
                .sample_rate(SAMPLE_RATE)
                .session(dispatcher)
                .timestretch(controls)
                .crossfade_duration(0.0)
                .build(),
        ));
        apply_mix([(player.as_ref(), signal.gain)])
            .expect("latency fixture linear signal level is valid");
        player
            .ensure_engine_started()
            .expect("latency deck engine starts");
        let queue = Arc::new(Queue::new(
            QueueConfig::builder()
                .player(Arc::clone(&player))
                .store(resources.store().clone())
                .build(),
        ));
        let events = Mutex::new(player.subscribe());
        let _ = queue.append(signal.source.clone());
        let deck = Self {
            block_frames,
            events,
            failures: Mutex::new(Vec::new()),
            player,
            queue,
            resources: resources.clone(),
            session,
            underruns: AtomicU64::new(0),
        };
        deck.set_session_tempo(SESSION_BPM);
        let _ = deck.pull().await;
        deck.queue.play();
        let mut audible = false;
        for _ in 0..LOAD_PULL_LIMIT {
            let pcm = deck.pull().await;
            audible |= pcm.iter().any(|sample| sample.abs() > 0.01);
            if audible && deck.player.is_playing() {
                return deck;
            }
        }
        deck.record_failure(
            "prepared WAV did not become audible within the manual-ring pull budget",
        );
        deck
    }

    pub(super) const fn resources(&self) -> &SyncFixtureResources {
        &self.resources
    }

    pub(super) fn bind(&self, analysis: &TrackAnalysis) -> Result<(), String> {
        let quantum = SyncQuantum::new(1.0).expect("one-beat quantum is valid");
        let map = analysis_map(analysis, "latency deck").map_err(|error| error.to_string())?;
        let result = self
            .queue
            .start_at_map(0, map, Beat::default(), quantum)
            .map_err(|error| error.to_string());
        if let Err(error) = &result {
            self.record_failure(format!("running deck rejected the sync target: {error}"));
        }
        result
    }

    pub(super) async fn pull(&self) -> Vec<f32> {
        if let Err(error) = self.session.credit(1) {
            self.record_failure(format!("manual-ring callback failed: {error}"));
        }
        let expected_samples = self.block_frames * usize::from(CHANNELS);
        let mut pcm = match self.session.drain(self.block_frames) {
            Ok(pcm) => pcm,
            Err(error) => {
                self.record_failure(format!("manual-ring callback drain failed: {error}"));
                Vec::new()
            }
        };
        if pcm.len() != expected_samples {
            self.record_failure(format!(
                "manual-ring callback returned {} samples, expected {expected_samples}",
                pcm.len(),
            ));
            pcm.resize(expected_samples, 0.0);
            pcm.truncate(expected_samples);
        }
        if let Err(error) = self.queue.tick() {
            self.record_failure(format!("latency queue tick failed: {error}"));
        }
        self.drain_events();
        task::yield_now().await;
        pcm
    }

    pub(super) async fn capture_blocks(&self, blocks: usize) -> Vec<f32> {
        let mut pcm = Vec::with_capacity(blocks * self.block_frames * usize::from(CHANNELS));
        for _ in 0..blocks {
            pcm.extend_from_slice(&self.pull().await);
        }
        pcm
    }

    pub(super) fn set_session_tempo(&self, bpm: f64) {
        let tempo = Tempo::new(bpm).expect("fixture session tempo is valid");
        match self
            .session
            .exec(Cmd::SetSessionTempo { tempo })
            .map_err(|error| error.to_string())
        {
            Ok(Reply::Ok) => {}
            Ok(Reply::Err(error)) => {
                self.record_failure(format!("session tempo command failed: {error}"));
            }
            Ok(_) => self.record_failure("unexpected session tempo reply"),
            Err(error) => self.record_failure(format!(
                "session tempo command could not reach the ring worker: {error}"
            )),
        }
    }

    fn host_map(&self) -> Option<BeatMapSnapshot> {
        match self
            .session
            .exec(Cmd::QuerySessionTransport)
            .map_err(|error| error.to_string())
        {
            Ok(Reply::SessionTransport(transport)) => Some(transport.host_map().snapshot()),
            Ok(Reply::Err(error)) => {
                self.record_failure(format!("session host-map query failed: {error}"));
                None
            }
            Ok(_) => {
                self.record_failure("unexpected session host-map query reply");
                None
            }
            Err(error) => {
                self.record_failure(format!(
                    "session host-map query could not reach the ring worker: {error}"
                ));
                None
            }
        }
    }

    pub(super) fn committed_frames(&self) -> u64 {
        match self.session.committed_frames() {
            Ok(frames) => frames,
            Err(error) => {
                self.record_failure(format!("manual-ring frame ledger failed: {error}"));
                0
            }
        }
    }

    pub(super) fn worker(&self) -> AudioWorkerHandle {
        self.player.worker().clone()
    }

    pub(super) fn underruns(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }

    pub(super) fn failures(&self) -> Vec<String> {
        self.failures.lock().clone()
    }

    fn record_failure(&self, failure: impl Into<String>) {
        let failure = failure.into();
        let mut failures = self.failures.lock();
        if !failures.contains(&failure) {
            failures.push(failure);
        }
    }

    fn drain_events(&self) {
        let mut events = self.events.lock();
        loop {
            match events.try_recv().map(|envelope| envelope.event) {
                Ok(Event::Decoder(DecoderEvent::DecodeError { kind, detail, .. })) => {
                    self.record_failure(format!(
                        "latency fixture decoder failed: {kind:?}: {detail}"
                    ));
                }
                Ok(Event::Audio(AudioEvent::UnderrunStarted { .. })) => {
                    self.underruns.fetch_add(1, Ordering::Relaxed);
                }
                Ok(_) => {}
                Err(TryRecvError::Lagged(skipped)) => {
                    self.record_failure(format!(
                        "latency fixture event receiver lagged by {skipped} event(s)"
                    ));
                }
                Err(TryRecvError::Closed) => {
                    self.record_failure("latency fixture event receiver closed during capture");
                    return;
                }
                Err(TryRecvError::Empty) => return,
            }
        }
    }
}

struct PcmRun {
    command_frame: u64,
    command_index: usize,
    failures: Vec<String>,
    samples: Vec<f32>,
    start_frame: u64,
}

struct AlignedPcm {
    candidate: Vec<f32>,
    candidate_start: usize,
    command_index: usize,
    control: Vec<f32>,
    lag_frames: i64,
    pre_rms: f64,
}

async fn retarget_run(
    resources: &SyncFixtureResources,
    signal: &SignalAsset,
    block_frames: usize,
    phase: TaggedSignalPhase,
    retarget: bool,
) -> PcmRun {
    let deck = RingDeck::load(resources, signal, block_frames).await;
    let _ = deck.bind(&analysis(0));
    for _ in 0..phase.warm_blocks() {
        let _ = deck.pull().await;
    }
    let start_frame = deck.committed_frames();
    let pre_blocks = (SAMPLE_RATE as usize).div_ceil(block_frames);
    let mut samples = deck.capture_blocks(pre_blocks).await;
    let command_index = samples.len() / usize::from(CHANNELS);
    let command_frame = deck.committed_frames();
    if retarget {
        deck.set_session_tempo(132.0);
    }
    let post_blocks = (SAMPLE_RATE as usize * 2).div_ceil(block_frames);
    for _ in 0..post_blocks {
        samples.extend_from_slice(&deck.pull().await);
    }
    PcmRun {
        command_frame,
        command_index,
        failures: deck.failures(),
        samples,
        start_frame,
    }
}

fn align_runs(candidate: &PcmRun, control: &PcmRun) -> AlignedPcm {
    let channels = usize::from(CHANNELS);
    let prefix = (SAMPLE_RATE as usize / 10).min(candidate.command_index / 2);
    let max_lag = candidate.command_index.saturating_sub(prefix);
    let mut best = (f64::INFINITY, 0_i64, 0_usize, 0_usize);
    for lag in -(max_lag as i64)..=max_lag as i64 {
        let candidate_start = usize::try_from((-lag).max(0)).expect("lag fits usize");
        let control_start = usize::try_from(lag.max(0)).expect("lag fits usize");
        let usable = candidate
            .command_index
            .saturating_sub(candidate_start)
            .min(control.command_index.saturating_sub(control_start));
        if usable < prefix || prefix == 0 {
            continue;
        }
        let offset = usable - prefix;
        let squared = (0..prefix)
            .step_by(16)
            .map(|frame| {
                let candidate_sample =
                    candidate.samples[(candidate_start + offset + frame) * channels];
                let control_sample = control.samples[(control_start + offset + frame) * channels];
                let delta = f64::from(candidate_sample - control_sample);
                delta * delta
            })
            .sum::<f64>();
        if squared < best.0 {
            best = (squared, lag, candidate_start, control_start);
        }
    }
    let (_, lag_frames, candidate_start, control_start) = best;
    let candidate_frames = candidate.samples.len() / channels - candidate_start;
    let control_frames = control.samples.len() / channels - control_start;
    let frames = candidate_frames.min(control_frames);
    let candidate_samples =
        &candidate.samples[candidate_start * channels..(candidate_start + frames) * channels];
    let control_samples =
        &control.samples[control_start * channels..(control_start + frames) * channels];
    let command_index = candidate.command_index - candidate_start;
    let pre_start = command_index.saturating_sub(SAMPLE_RATE as usize / 10);
    let pre_rms = frame_deltas(candidate_samples, control_samples)
        .skip(pre_start)
        .take(command_index - pre_start)
        .map(|delta| f64::from(delta) * f64::from(delta))
        .sum::<f64>()
        / (command_index - pre_start)
            .max(1)
            .to_f64()
            .expect("pre-command window fits f64");
    AlignedPcm {
        candidate: candidate_samples.to_vec(),
        candidate_start,
        command_index,
        control: control_samples.to_vec(),
        lag_frames,
        pre_rms: pre_rms.sqrt(),
    }
}

fn frame_deltas<'a>(candidate: &'a [f32], control: &'a [f32]) -> impl Iterator<Item = f32> + 'a {
    let channels = usize::from(CHANNELS);
    candidate
        .chunks_exact(channels)
        .zip(control.chunks_exact(channels))
        .map(|(candidate, control)| {
            candidate
                .iter()
                .zip(control)
                .map(|(candidate, control)| (candidate - control).abs())
                .fold(0.0_f32, f32::max)
        })
}

fn first_sustained_delta(
    candidate: &[f32],
    control: &[f32],
    range: std::ops::Range<usize>,
) -> Option<usize> {
    const DELTA_THRESHOLD: f32 = 0.002;
    const SUSTAINED_FRAMES: usize = 32;
    let mut run = 0_usize;
    for (frame, delta) in frame_deltas(candidate, control).enumerate() {
        if !range.contains(&frame) {
            continue;
        }
        if delta > DELTA_THRESHOLD {
            run += 1;
            if run == SUSTAINED_FRAMES {
                return Some(frame + 1 - SUSTAINED_FRAMES);
            }
        } else {
            run = 0;
        }
    }
    None
}

#[kithara::test(tokio, multi_thread)]
async fn latency_signals_are_eager_prepared_and_range_readable() {
    let fixture = SyncFixture::new(sync_fixture_resources("latency-signal-preparation")).await;
    let client = Client::new();

    for signal in [fixture.chirp(), fixture.pulse()] {
        let response = client
            .get(signal.uri())
            .header(RANGE, "bytes=0-43")
            .send()
            .await
            .expect("prepared signal range request succeeds");
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        let bytes = response
            .bytes()
            .await
            .expect("prepared signal range response is readable");
        assert_eq!(&bytes[..4], b"RIFF");
    }
}

#[kithara::test(tokio, multi_thread, timeout(Duration::from_secs(300)))]
async fn latency_signals_preserve_levels_through_player_and_queue() {
    let fixture = SyncFixture::new(sync_fixture_resources("latency-signal-levels")).await;

    assert_player_peak(fixture.resources(), fixture.chirp(), CHIRP_PEAK).await;
    assert_player_peak(fixture.resources(), fixture.pulse(), PULSE_PEAK).await;
}

async fn assert_player_peak(resources: &SyncFixtureResources, signal: &SignalAsset, expected: f32) {
    const BLOCK_FRAMES: usize = 256;
    const PEAK_TOLERANCE: f32 = 0.02;

    let deck = RingDeck::load(resources, signal, BLOCK_FRAMES).await;
    let blocks = (SAMPLE_RATE as usize).div_ceil(BLOCK_FRAMES);
    let pcm = deck.capture_blocks(blocks).await;
    let peak = pcm.iter().copied().map(f32::abs).fold(0.0, f32::max);

    assert!(
        deck.failures().is_empty(),
        "prepared signal failed in Player/Queue: {:?}",
        deck.failures(),
    );
    assert!(
        (peak - expected).abs() <= PEAK_TOLERANCE,
        "Player/Queue peak {peak:.6} differs from expected {expected:.6}",
    );
}

#[kithara::test(tokio, multi_thread, serial, timeout(Duration::from_secs(300)))]
#[ignore = "SYNC-ORACLE SYNC-PRODUCT-LATENCY-001: waiting for Wave ResidentPlan"]
async fn bound_tempo_retarget_reaches_pcm_within_twenty_ms() {
    let fixture = SyncFixture::new(sync_fixture_resources("latency-bound-retarget")).await;
    for block_frames in [128_usize, 256, 512] {
        for phase in TaggedSignalPhase::ALL {
            let control = retarget_run(
                fixture.resources(),
                fixture.chirp(),
                block_frames,
                phase,
                false,
            )
            .await;
            let control_report = CochleaReport::measure(&control.samples, CHANNELS, SAMPLE_RATE);
            let candidate = retarget_run(
                fixture.resources(),
                fixture.chirp(),
                block_frames,
                phase,
                true,
            )
            .await;
            let aligned = align_runs(&candidate, &control);
            let candidate_report =
                CochleaReport::measure(&aligned.candidate, CHANNELS, SAMPLE_RATE);
            let aligned_control_report =
                CochleaReport::measure(&aligned.control, CHANNELS, SAMPLE_RATE);
            let mut failures = continuity_failures(
                "bound tempo retarget",
                &candidate_report,
                &aligned_control_report,
            );
            failures.extend(
                control
                    .failures
                    .iter()
                    .map(|failure| format!("control: {failure}")),
            );
            failures.extend(candidate.failures.iter().cloned());
            if !candidate.samples.iter().any(|sample| sample.abs() > 0.01) {
                failures.push("retarget fixture produced no audible PCM".to_owned());
            }
            if aligned.pre_rms > 0.002 {
                failures.push(format!(
                    "time-aligned pre-command RMS delta {:.6} exceeds 0.002",
                    aligned.pre_rms,
                ));
            }
            if let Some(frame) = first_sustained_delta(
                &aligned.candidate,
                &aligned.control,
                0..aligned.command_index,
            ) {
                failures.push(format!(
                    "candidate diverged from control before retarget at aligned frame {frame}",
                ));
            }
            let transition = first_sustained_delta(
                &aligned.candidate,
                &aligned.control,
                aligned.command_index..aligned.candidate.len() / usize::from(CHANNELS),
            );
            let latency_frames = transition.map(|frame| frame - aligned.command_index);
            let latency_budget = TWENTY_MS_FRAMES.min(block_frames * 2);
            match latency_frames {
                Some(frames) if frames <= latency_budget => {}
                Some(frames) => failures.push(format!(
                    "retarget first changed PCM after {frames} frames; budget is {latency_budget} (two callbacks and no more than 20 ms)",
                )),
                None => failures.push("retarget produced no sustained PCM change".to_owned()),
            }
            let case = format!("bound-retarget-{block_frames}-{}", phase.label(),);
            let mut metadata = SyncArtifactMetadata::new(case, SAMPLE_RATE, CHANNELS, block_frames);
            metadata.add_source(ArtifactSource::new("deck-a", fixture.chirp().uri()));
            metadata.set_operation(format!(
                "bound 120->132 BPM retarget at {} tagged-signal phase",
                phase.label(),
            ));
            metadata.add_frame(ArtifactFrame::new(candidate.start_frame, "capture-start"));
            metadata.add_frame(ArtifactFrame::new(
                candidate.command_frame,
                "retarget-command",
            ));
            if let Some(frame) = transition {
                metadata.add_frame(ArtifactFrame::new(
                    candidate
                        .start_frame
                        .saturating_add(u64::try_from(aligned.candidate_start).unwrap_or(u64::MAX))
                        .saturating_add(u64::try_from(frame).unwrap_or(u64::MAX)),
                    "first-sustained-pcm-change",
                ));
                metadata.add_state(
                    "transition_position_in_block",
                    ((aligned.candidate_start + frame) % block_frames).to_string(),
                );
            }
            metadata.add_state("alignment_lag_frames", aligned.lag_frames.to_string());
            metadata.add_threshold(
                "absolute_command_to_audible_frames",
                TWENTY_MS_FRAMES.to_f64().expect("latency budget fits f64"),
            );
            metadata.add_threshold(
                "effective_command_to_audible_frames",
                latency_budget.to_f64().expect("latency budget fits f64"),
            );
            metadata.add_threshold("plan_to_pcm_blocks", 2.0);
            metadata.add_threshold("pre_command_rms", 0.002);
            metadata.add_failures(failures.clone());
            write_sync_artifact(
                &metadata,
                &[
                    ArtifactAudio::new("deck-a-stem", &aligned.candidate),
                    ArtifactAudio::new("final-mix", &aligned.candidate),
                    ArtifactAudio::new("time-aligned-control", &aligned.control),
                ],
            )
            .expect("optional sync latency artifact writes before assertions");

            assert_oracle_load_bearing(&aligned.control, CHANNELS, SAMPLE_RATE, 512);
            assert!(
                failures.is_empty(),
                "{block_frames}/{} latency contract failed: {}\nraw_control={control_report:?}\naligned_control={aligned_control_report:?}\ncandidate={candidate_report:?}",
                phase.label(),
                failures.join("; "),
            );
        }
    }
}

#[kithara::test(tokio, multi_thread, serial, timeout(Duration::from_secs(300)))]
#[ignore = "SYNC-ORACLE SYNC-PRODUCT-LATENCY-002: waiting for Wave ResidentPlan"]
async fn running_sync_command_changes_audible_pcm_within_one_block() {
    let fixture = SyncFixture::new(sync_fixture_resources("latency-running-sync")).await;
    for block_frames in [128_usize, 256, 512] {
        let control_deck = RingDeck::load(fixture.resources(), fixture.pulse(), block_frames).await;
        let pre_blocks = (SAMPLE_RATE as usize).div_ceil(block_frames);
        let mut control_samples = control_deck.capture_blocks(pre_blocks).await;
        let control_command_index = control_samples.len() / usize::from(CHANNELS);
        control_samples.extend_from_slice(
            &control_deck
                .capture_blocks((SAMPLE_RATE as usize * 2).div_ceil(block_frames))
                .await,
        );
        let control = PcmRun {
            command_frame: control_deck.committed_frames(),
            command_index: control_command_index,
            failures: control_deck.failures(),
            samples: control_samples,
            start_frame: 0,
        };
        let candidate_deck =
            RingDeck::load(fixture.resources(), fixture.pulse(), block_frames).await;
        let mut candidate = candidate_deck.capture_blocks(pre_blocks).await;
        let command_index = candidate.len() / usize::from(CHANNELS);
        let command_frame = candidate_deck.committed_frames();
        let command = candidate_deck.bind(&analysis(0));
        candidate.extend_from_slice(
            &candidate_deck
                .capture_blocks((SAMPLE_RATE as usize * 2).div_ceil(block_frames))
                .await,
        );
        let aligned = align_runs(
            &PcmRun {
                command_frame,
                command_index,
                failures: candidate_deck.failures(),
                samples: candidate,
                start_frame: command_frame.saturating_sub(u64::from(SAMPLE_RATE)),
            },
            &control,
        );
        let control_report = CochleaReport::measure(&aligned.control, CHANNELS, SAMPLE_RATE);
        let candidate_report = CochleaReport::measure(&aligned.candidate, CHANNELS, SAMPLE_RATE);
        let mut failures = Vec::new();
        failures.extend(
            control
                .failures
                .iter()
                .map(|failure| format!("control: {failure}")),
        );
        failures.extend(candidate_deck.failures());
        if let Err(error) = command {
            failures.push(format!("running SYNC command was rejected: {error}"));
        }
        let transition = first_sustained_delta(
            &aligned.candidate,
            &aligned.control,
            aligned.command_index..aligned.candidate.len() / usize::from(CHANNELS),
        );
        match transition.map(|frame| frame - aligned.command_index) {
            Some(frames) if frames <= block_frames => {}
            Some(frames) => failures.push(format!(
                "running SYNC first changed audible PCM after {frames} frames; budget is one {block_frames}-frame callback",
            )),
            None => failures.push("running SYNC produced no sustained audible PCM change".to_owned()),
        }
        failures.extend(continuity_failures(
            "running SYNC",
            &candidate_report,
            &control_report,
        ));

        let mut metadata = SyncArtifactMetadata::new(
            format!("running-sync-command-{block_frames}"),
            SAMPLE_RATE,
            CHANNELS,
            block_frames,
        );
        metadata.add_source(ArtifactSource::new("deck-a", fixture.pulse().uri()));
        metadata.set_operation("issue SYNC to an audible running Queue/Player deck");
        metadata.add_frame(ArtifactFrame::new(command_frame, "sync-command"));
        if let Some(frame) = transition {
            metadata.add_frame(ArtifactFrame::new(
                command_frame.saturating_add(
                    u64::try_from(frame.saturating_sub(aligned.command_index)).unwrap_or(u64::MAX),
                ),
                "first-sustained-pcm-change",
            ));
        }
        metadata.add_threshold(
            "command_to_audible_frames",
            block_frames.to_f64().expect("block size fits f64"),
        );
        metadata.add_failures(failures.clone());
        write_sync_artifact(
            &metadata,
            &[
                ArtifactAudio::new("deck-a-stem", &aligned.candidate),
                ArtifactAudio::new("final-mix", &aligned.candidate),
                ArtifactAudio::new("time-aligned-no-sync-control", &aligned.control),
            ],
        )
        .expect("optional running SYNC artifact writes before assertions");

        assert_oracle_load_bearing(&aligned.control, CHANNELS, SAMPLE_RATE, 512);
        assert!(
            failures.is_empty(),
            "running SYNC {block_frames}-frame contract failed: {}\ncontrol={control_report:?}\ncandidate={candidate_report:?}",
            failures.join("; "),
        );
    }
}

struct PhaseCapture {
    host_map: Option<BeatMapSnapshot>,
    failures: Vec<String>,
    samples: Vec<f32>,
    start_frame: u64,
}

async fn capture_sync_sequence(
    resources: &SyncFixtureResources,
    signal: &SignalAsset,
    offsets: &[u64],
) -> PhaseCapture {
    const BLOCK_FRAMES: usize = 512;
    let deck = RingDeck::load(resources, signal, BLOCK_FRAMES).await;
    for offset in offsets {
        let _ = deck.bind(&analysis(*offset));
    }
    let _ = deck.capture_blocks(4).await;
    let start_frame = deck.committed_frames();
    let host_map = deck.host_map();
    let blocks = (SAMPLE_RATE as usize * 6).div_ceil(BLOCK_FRAMES);
    let samples = deck.capture_blocks(blocks).await;
    PhaseCapture {
        host_map,
        failures: deck.failures(),
        samples,
        start_frame,
    }
}

#[kithara::test(tokio, multi_thread, serial, timeout(Duration::from_secs(300)))]
#[ignore = "SYNC-ORACLE SYNC-PRODUCT-LATENCY-003: waiting for Wave ResidentPlan"]
async fn latest_sync_target_wins_in_pcm() {
    const BLOCK_FRAMES: usize = 512;
    let fixture = SyncFixture::new(sync_fixture_resources("latency-latest-target")).await;
    let beat = beat_frames(SAMPLE_RATE) as u64;
    let first = beat / 3;
    let second = beat * 2 / 3;

    let first_control = capture_sync_sequence(fixture.resources(), fixture.pulse(), &[first]).await;
    let second_control =
        capture_sync_sequence(fixture.resources(), fixture.pulse(), &[second]).await;
    let latest_control = capture_sync_sequence(fixture.resources(), fixture.pulse(), &[0]).await;
    let latest_report = CochleaReport::measure(&latest_control.samples, CHANNELS, SAMPLE_RATE);
    let candidate =
        capture_sync_sequence(fixture.resources(), fixture.pulse(), &[first, second, 0]).await;
    let candidate_report = CochleaReport::measure(&candidate.samples, CHANNELS, SAMPLE_RATE);
    let mut failures = continuity_failures("latest target", &candidate_report, &latest_report);
    for (label, capture) in [
        ("first control", &first_control),
        ("second control", &second_control),
        ("latest control", &latest_control),
        ("candidate", &candidate),
    ] {
        failures.extend(
            capture
                .failures
                .iter()
                .map(|failure| format!("{label}: {failure}")),
        );
    }

    let candidate_phase = absolute_onset_phases(&candidate);
    let first_phase = absolute_onset_phases(&first_control);
    let second_phase = absolute_onset_phases(&second_control);
    let latest_phase = absolute_onset_phases(&latest_control);
    let latest_distance = phase_distance(&candidate_phase, &latest_phase);
    let first_distance = phase_distance(&candidate_phase, &first_phase);
    let second_distance = phase_distance(&candidate_phase, &second_phase);
    match latest_distance {
        Some(distance) if distance <= BLOCK_FRAMES => {}
        Some(distance) => failures.push(format!(
            "candidate is {distance} frames from the latest target; budget is {BLOCK_FRAMES}",
        )),
        None => failures.push("candidate/latest PCM has too few tagged onsets".to_owned()),
    }
    for (label, distance) in [("first", first_distance), ("second", second_distance)] {
        match (latest_distance, distance) {
            (Some(latest), Some(stale)) if stale > latest + BLOCK_FRAMES => {}
            (Some(latest), Some(stale)) => failures.push(format!(
                "candidate does not prefer latest over {label} target: latest={latest}, stale={stale}",
            )),
            _ => failures.push(format!("{label} target PCM has too few tagged onsets")),
        }
    }

    let mut metadata = SyncArtifactMetadata::new(
        "latest-sync-target-pcm",
        SAMPLE_RATE,
        CHANNELS,
        BLOCK_FRAMES,
    );
    metadata.add_source(ArtifactSource::new("deck-a", fixture.pulse().uri()));
    metadata.set_operation("publish stale-A, stale-B, latest-C before the next render callback");
    metadata.add_frame(ArtifactFrame::new(
        candidate.start_frame,
        "candidate-capture-start",
    ));
    metadata.add_state("latest_phase_distance", format!("{latest_distance:?}"));
    metadata.add_state("first_phase_distance", format!("{first_distance:?}"));
    metadata.add_state("second_phase_distance", format!("{second_distance:?}"));
    metadata.add_threshold(
        "latest_target_phase_frames",
        BLOCK_FRAMES.to_f64().expect("block size fits f64"),
    );
    metadata.add_failures(failures.clone());
    write_sync_artifact(
        &metadata,
        &[
            ArtifactAudio::new("deck-a-stem", &candidate.samples),
            ArtifactAudio::new("final-mix", &candidate.samples),
            ArtifactAudio::new("time-aligned-control", &latest_control.samples),
            ArtifactAudio::new("stale-first-control", &first_control.samples),
            ArtifactAudio::new("stale-second-control", &second_control.samples),
        ],
    )
    .expect("optional latest-target artifact writes before assertions");

    assert_oracle_load_bearing(&latest_control.samples, CHANNELS, SAMPLE_RATE, BLOCK_FRAMES);
    assert!(
        failures.is_empty(),
        "latest target PCM contract failed: {}\nlatest={latest_report:?}\ncandidate={candidate_report:?}",
        failures.join("; "),
    );
}

fn beat_frames(sample_rate: u32) -> usize {
    (f64::from(sample_rate) * 60.0 / SESSION_BPM)
        .round()
        .to_usize()
        .expect("beat length fits usize")
}

pub(super) fn analysis(marker_offset: u64) -> TrackAnalysis {
    let rate = NonZeroU32::new(SAMPLE_RATE).expect("fixture sample rate is non-zero");
    let beat = beat_frames(SAMPLE_RATE) as u64;
    let markers = (0..)
        .map(|index| marker_offset + index * beat)
        .take_while(|marker| *marker < TRACK_FRAMES as u64)
        .collect::<Vec<_>>();
    let downbeats = markers.iter().step_by(4).copied().collect();
    TrackAnalysis::with_source_rate(
        Some(BeatGrid::new(SESSION_BPM, markers, downbeats, Vec::new())),
        None,
        TRACK_FRAMES as u64,
        rate,
    )
}

fn pulse_onsets(samples: &[f32]) -> Vec<usize> {
    const THRESHOLD: f32 = 0.02;
    let channels = usize::from(CHANNELS);
    let refractory = beat_frames(SAMPLE_RATE) / 2;
    let mut onsets = Vec::new();
    let mut was_loud = false;
    for (frame, samples) in samples.chunks_exact(channels).enumerate() {
        let loud = samples.iter().any(|sample| sample.abs() >= THRESHOLD);
        if loud
            && !was_loud
            && onsets
                .last()
                .is_none_or(|last| frame.saturating_sub(*last) >= refractory)
        {
            onsets.push(frame);
        }
        was_loud = loud;
    }
    onsets
}

fn absolute_onset_phases(capture: &PhaseCapture) -> Vec<usize> {
    let Some(host_map) = capture.host_map.as_ref() else {
        return Vec::new();
    };
    let beat = beat_frames(SAMPLE_RATE);
    pulse_onsets(&capture.samples)
        .into_iter()
        .filter_map(|onset| {
            let absolute = capture
                .start_frame
                .checked_add(u64::try_from(onset).ok()?)?;
            let frame = SessionFrame::new(i64::try_from(absolute).ok()?);
            let resolved =
                host_map.beat_at(MapPoint::new(host_map.stamp(), MapPosition::Host(frame)));
            let MapQuery::Resolved(resolved) = resolved else {
                return None;
            };
            let phase = f64::from(*resolved.value().value()).rem_euclid(1.0)
                * beat.to_f64().expect("beat length fits f64");
            phase.round().to_usize().map(|phase| phase % beat)
        })
        .collect()
}

fn phase_distance(left: &[usize], right: &[usize]) -> Option<usize> {
    let beat = beat_frames(SAMPLE_RATE);
    let mut distances = left
        .iter()
        .filter_map(|left| {
            right
                .iter()
                .map(|right| {
                    let direct = left.abs_diff(*right);
                    direct.min(beat - direct)
                })
                .min()
        })
        .collect::<Vec<_>>();
    if distances.len() < 3 {
        return None;
    }
    distances.sort_unstable();
    Some(distances[distances.len() / 2])
}
