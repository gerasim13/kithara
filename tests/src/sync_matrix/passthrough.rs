use anyhow::{Context, Result, bail};
use kithara::{
    audio::{Audio, AudioConfig, EngineLoad, PcmSession, StretchControls, StretchKind, TempoSlot},
    events::TrackStatus,
    hls::AbrMode,
    platform::{
        sync::Arc,
        time::{self, Duration, Instant},
    },
    stream::Stream,
};
use num_traits::ToPrimitive;

use super::{
    CHANNELS, CaptureSource, PassthroughProfile, PcmCapture, PlaybackMode, RENDER_FRAMES, SyncCase,
    SyncHarness, SyncMedia,
};
use crate::{
    cochlea::{CochleaReport, assert_rhythmic_oracle_load_bearing, continuity_failures},
    create_test_wav,
    memory_source::{MemStream, MemStreamConfig, MemorySource},
    offline::{OfflinePlayer, resource_from_reader},
    sync_artifact::{
        ArtifactAudio, ArtifactFrame, ArtifactSource, SyncArtifactMetadata, write_sync_artifact,
    },
};

const LOAD_PLAYERS: usize = 4;
const LOAD_SECONDS: usize = 12;
const LOAD_SPEED: f32 = 0.5;
const PASSTHROUGH_SETTLE_PULL_LIMIT: usize = 4_000;
const SHARED_WORKER_WARMUP_BLOCKS: usize = 32;

struct SharedWorkerLoad {
    meter: Arc<EngineLoad>,
    players: Vec<OfflinePlayer>,
}

impl SharedWorkerLoad {
    fn is_observed(&self) -> bool {
        self.meter.snapshot().is_active()
    }

    fn render(&mut self, frames: usize) {
        for player in &mut self.players {
            let _ = player.render(frames);
        }
    }
}

#[derive(Clone, Debug)]
struct PassthroughDeckOutcome {
    current_index: Option<usize>,
    event_lagged: u64,
    event_stream_closed: bool,
    playing: bool,
    rate: f32,
    sync_index: Option<usize>,
    track_failed: bool,
    underruns: usize,
}

#[derive(Clone, Debug)]
struct PassthroughRun {
    abr_switch_failures: usize,
    abr_switches: usize,
    abr_switches_expected: usize,
    capture_failures: Vec<String>,
    decks: Vec<PassthroughDeckOutcome>,
    mix: PcmCapture,
    load_observed: bool,
    reloads: usize,
    session_bpm: f64,
    stems: Vec<PcmCapture>,
}

#[derive(Clone, Debug)]
struct PassthroughBundle {
    candidate: PassthroughRun,
    control: PassthroughRun,
    library_seed: Option<u64>,
    media_id: String,
    sources: Vec<CaptureSource>,
}

impl SyncHarness {
    async fn capture_passthrough(mut self, prefix: &str) -> Result<PassthroughRun> {
        self.start_initial().await?;
        self.apply_uniform_gain()?;
        let mut load = None;
        self.render_paced_blocks(4, &mut load).await?;
        let mix = self
            .capture_paced_mix(format!("{prefix}-mix"), &mut load)
            .await?;
        let mut stems = Vec::with_capacity(self.case.decks);
        for audible_deck in 0..self.case.decks {
            self.apply_gain_mask(audible_deck)?;
            self.render_paced_blocks(4, &mut load).await?;
            stems.push(
                self.capture_paced_mix(format!("{prefix}-deck-{audible_deck}"), &mut load)
                    .await?,
            );
        }
        self.finish_passthrough_run(mix, stems, false)
    }

    async fn exercise_passthrough_lifecycle(&mut self) -> Result<()> {
        let mut load = None;
        for deck_index in 0..self.decks.len() {
            self.decks[deck_index]
                .queue
                .seek(self.case.seek_seconds)
                .with_context(|| format!("{}: passthrough seek deck {deck_index}", self.case))?;
            self.record("passthrough-seek");
            self.render_paced_blocks(8, &mut load).await?;
        }
        self.reload_all_passthrough(&mut load).await?;
        self.switch_abr_variants_passthrough(&mut load).await
    }

    async fn reload_all_passthrough(&mut self, load: &mut Option<SharedWorkerLoad>) -> Result<()> {
        for deck_index in 0..self.decks.len() {
            let reload_id = self.decks[deck_index].reload_id;
            let reload_index = self.decks[deck_index]
                .queue
                .tracks()
                .iter()
                .position(|track| track.id == reload_id)
                .with_context(|| format!("{}: passthrough reload track is absent", self.case))?;
            self.decks[deck_index]
                .queue
                .select(reload_id, kithara::queue::Transition::None)
                .with_context(|| format!("{}: passthrough reload deck {deck_index}", self.case))?;
            self.reloads = self.reloads.saturating_add(1);
            self.record("passthrough-reload");
            self.wait_deck_index_passthrough(deck_index, reload_index, load)
                .await?;
        }
        self.render_paced_blocks(8, load).await?;
        Ok(())
    }

    async fn wait_deck_index_passthrough(
        &mut self,
        deck_index: usize,
        expected: usize,
        load: &mut Option<SharedWorkerLoad>,
    ) -> Result<()> {
        for _ in 0..PASSTHROUGH_SETTLE_PULL_LIMIT {
            let landed = self.decks.get(deck_index).is_some_and(|deck| {
                deck.queue.current_index() == Some(expected) && deck.player.is_playing()
            });
            if landed {
                self.record("passthrough-reload-index-landed");
                return Ok(());
            }
            self.render_paced_block(RENDER_FRAMES, load).await?;
        }
        let observed = self
            .decks
            .get(deck_index)
            .and_then(|deck| deck.queue.current_index());
        bail!(
            "{}: passthrough deck {deck_index} reload did not land at queue index {expected}; observed={observed:?}",
            self.case,
        )
    }

    async fn switch_abr_variants_passthrough(
        &mut self,
        load: &mut Option<SharedWorkerLoad>,
    ) -> Result<()> {
        let mut requests = Vec::new();
        for deck_index in 0..self.decks.len() {
            let Some(target) = self.decks[deck_index].abr_target else {
                continue;
            };
            self.decks[deck_index].abr_applied_target = None;
            self.decks[deck_index].abr_wait_target = Some(target);
            let requested = self.decks[deck_index]
                .player
                .current_abr_handle()
                .is_some_and(|handle| handle.set_mode(AbrMode::manual(target)).is_ok());
            requests.push((deck_index, target, requested));
        }
        if !requests.is_empty() {
            self.record("passthrough-abr-switch");
            for _ in 0..PASSTHROUGH_SETTLE_PULL_LIMIT {
                self.render_paced_block(RENDER_FRAMES, load).await?;
                let all_applied = requests.iter().all(|(deck_index, target, requested)| {
                    !requested || self.decks[*deck_index].abr_applied_target == Some(*target)
                });
                if all_applied {
                    break;
                }
            }
        }
        for (deck_index, target, requested) in requests {
            if requested && self.decks[deck_index].abr_applied_target == Some(target) {
                self.abr_switches = self.abr_switches.saturating_add(1);
            } else {
                self.abr_switch_failures = self.abr_switch_failures.saturating_add(1);
            }
        }
        Ok(())
    }

    async fn capture_passthrough_lifecycle(mut self, prefix: &str) -> Result<PassthroughRun> {
        self.start_initial().await?;
        self.apply_uniform_gain()?;
        let (start_session_frame, mut tap) = self.start_pcm_capture()?;
        if let Err(error) = self.exercise_passthrough_lifecycle().await {
            self.capture_failures
                .push(format!("passthrough lifecycle: {error:#}"));
        }
        let mut load = None;
        if let Err(error) = self
            .render_paced_frames(self.case.capture_frames(), &mut load)
            .await
        {
            self.capture_failures
                .push(format!("passthrough lifecycle tail: {error:#}"));
        }
        let mix = self.finish_pcm_capture(
            format!("{prefix}-lifecycle-mix"),
            start_session_frame,
            &mut tap,
        );
        let mut stems = Vec::with_capacity(self.case.decks);
        for audible_deck in 0..self.case.decks {
            self.apply_gain_mask(audible_deck)?;
            self.render_paced_blocks(4, &mut load).await?;
            stems.push(
                self.capture_paced_mix(
                    format!("{prefix}-lifecycle-deck-{audible_deck}"),
                    &mut load,
                )
                .await?,
            );
        }
        self.finish_passthrough_run(mix, stems, false)
    }

    fn finish_passthrough_run(
        self,
        mix: PcmCapture,
        stems: Vec<PcmCapture>,
        load_observed: bool,
    ) -> Result<PassthroughRun> {
        let decks = self
            .decks
            .iter()
            .map(|deck| PassthroughDeckOutcome {
                current_index: deck.queue.current_index(),
                event_lagged: deck.event_lagged,
                event_stream_closed: deck.event_stream_closed,
                playing: deck.player.is_playing(),
                rate: deck.player.rate(),
                sync_index: deck.sync_index,
                track_failed: deck
                    .queue
                    .tracks()
                    .iter()
                    .any(|track| matches!(track.status, TrackStatus::Failed(_))),
                underruns: deck.underruns,
            })
            .collect();
        let session_bpm = self.current_session_bpm()?;
        Ok(PassthroughRun {
            abr_switch_failures: self.abr_switch_failures,
            abr_switches: self.abr_switches,
            abr_switches_expected: self.abr_switches_expected,
            capture_failures: self.capture_failures,
            decks,
            mix,
            load_observed,
            reloads: self.reloads,
            session_bpm,
            stems,
        })
    }

    async fn capture_passthrough_shared_worker(
        mut self,
        prefix: &str,
        with_load: bool,
    ) -> Result<PassthroughRun> {
        self.start_initial().await?;
        self.apply_uniform_gain()?;
        let mut load = if with_load {
            Some(shared_worker_load(&self).await?)
        } else {
            None
        };
        self.render_paced_blocks(SHARED_WORKER_WARMUP_BLOCKS, &mut load)
            .await?;
        let mix = self
            .capture_paced_mix(format!("{prefix}-mix"), &mut load)
            .await?;
        let mut stems = Vec::with_capacity(self.case.decks);
        for audible_deck in 0..self.case.decks {
            self.apply_gain_mask(audible_deck)?;
            self.render_paced_blocks(4, &mut load).await?;
            stems.push(
                self.capture_paced_mix(format!("{prefix}-deck-{audible_deck}"), &mut load)
                    .await?,
            );
        }
        let load_observed = load.as_ref().is_some_and(SharedWorkerLoad::is_observed);
        self.finish_passthrough_run(mix, stems, load_observed)
    }

    async fn capture_paced_mix(
        &mut self,
        label: impl Into<String>,
        load: &mut Option<SharedWorkerLoad>,
    ) -> Result<PcmCapture> {
        let (start_session_frame, mut tap) = self.start_pcm_capture()?;
        let frames = self.case.capture_frames();
        if let Err(error) = self.render_paced_frames(frames, load).await {
            self.capture_failures
                .push(format!("passthrough paced capture: {error:#}"));
        }
        Ok(self.finish_pcm_capture(label, start_session_frame, &mut tap))
    }

    async fn render_paced_frames(
        &mut self,
        frames: usize,
        load: &mut Option<SharedWorkerLoad>,
    ) -> Result<()> {
        let full_blocks = frames / RENDER_FRAMES;
        let remainder = frames % RENDER_FRAMES;
        self.render_paced_blocks(full_blocks, load).await?;
        if remainder > 0 {
            self.render_paced_block(remainder, load).await?;
        }
        Ok(())
    }

    async fn render_paced_blocks(
        &mut self,
        blocks: usize,
        load: &mut Option<SharedWorkerLoad>,
    ) -> Result<()> {
        for _ in 0..blocks {
            self.render_paced_block(RENDER_FRAMES, load).await?;
        }
        Ok(())
    }

    async fn render_paced_block(
        &mut self,
        frames: usize,
        load: &mut Option<SharedWorkerLoad>,
    ) -> Result<()> {
        let started = Instant::now();
        if let Some(load) = load.as_mut() {
            load.render(frames);
        }
        let _ = self.render_block(frames)?;
        let period = Duration::from_secs_f64(
            frames.to_f64().context("render frames fit f64")? / f64::from(self.case.sample_rate),
        );
        time::sleep(period.saturating_sub(started.elapsed())).await;
        Ok(())
    }
}

async fn shared_worker_load(harness: &SyncHarness) -> Result<SharedWorkerLoad> {
    let meter = Arc::new(EngineLoad::default());
    let mut players = Vec::with_capacity(LOAD_PLAYERS);
    for _ in 0..LOAD_PLAYERS {
        let stream = MemStreamConfig {
            source: Some(MemorySource::new(create_test_wav(
                harness.case.sample_rate as usize * LOAD_SECONDS,
                harness.case.sample_rate,
                CHANNELS,
            ))),
            event_bus: None,
        };
        let controls = StretchControls::new(LOAD_SPEED);
        controls.set_backend(StretchKind::Signalsmith);
        controls.set_keylock(true);
        let config = AudioConfig::<MemStream>::for_stream(stream)
            .byte_pool(kithara::bufpool::BytePool::default())
            .pcm_pool(kithara::bufpool::PcmPool::default())
            .worker(harness.decks[0].player.worker().clone())
            .tempo(TempoSlot::from(controls))
            .engine_load(Arc::clone(&meter))
            .hint("wav".to_owned())
            .build();
        let mut audio = Audio::<Stream<MemStream>>::new(config)
            .await
            .context("construct passthrough shared-worker load")?;
        let gate = audio
            .preload_gate()
            .context("shared-worker load exposes preload gate")?;
        time::timeout(
            Duration::from_secs(5),
            gate.wait_for_epoch(audio.preload_epoch()),
        )
        .await
        .context("shared-worker load preload gate opens")?;
        audio.preload().context("preload shared-worker load")?;
        let mut player = OfflinePlayer::new(harness.case.sample_rate);
        player.set_fade_duration(0.0);
        player.load_and_fadein(
            resource_from_reader(audio),
            "passthrough-shared-worker-load",
        );
        players.push(player);
    }
    Ok(SharedWorkerLoad { meter, players })
}

fn passthrough_profile(case: SyncCase) -> Result<PassthroughProfile> {
    match case.playback {
        PlaybackMode::Passthrough(profile) => Ok(profile),
        PlaybackMode::Sync => bail!("{case}: passthrough runner requires passthrough playback"),
    }
}

async fn run_passthrough_row(case: SyncCase, media: SyncMedia) -> Result<PassthroughBundle> {
    let profile = passthrough_profile(case)?;
    let candidate = capture_profile(
        SyncHarness::open(case, media.clone()).await?,
        profile,
        "passthrough-candidate",
        true,
    )
    .await?;
    let control = capture_profile(
        SyncHarness::open(case, media.clone()).await?,
        profile,
        "passthrough-control",
        false,
    )
    .await?;
    let sources = (0..case.decks)
        .map(|deck| {
            let track = media.for_deck(deck);
            CaptureSource {
                analysis_key: track.analysis_key.clone(),
                deck: format!("deck-{deck}"),
                media: track.label.clone(),
            }
        })
        .collect();
    Ok(PassthroughBundle {
        candidate,
        control,
        library_seed: media.library_seed,
        media_id: media.id,
        sources,
    })
}

async fn run_synthetic_passthrough_row(case: SyncCase) -> Result<PassthroughBundle> {
    let profile = passthrough_profile(case)?;
    let candidate = capture_profile(
        SyncHarness::synthetic(case).await?,
        profile,
        "passthrough-candidate",
        true,
    )
    .await?;
    let control = capture_profile(
        SyncHarness::synthetic(case).await?,
        profile,
        "passthrough-control",
        false,
    )
    .await?;
    let sources = (0..case.decks)
        .map(|deck| CaptureSource {
            analysis_key: format!("synthetic-deck-{deck}"),
            deck: format!("deck-{deck}"),
            media: "synthetic-pulse".to_owned(),
        })
        .collect();
    Ok(PassthroughBundle {
        candidate,
        control,
        library_seed: None,
        media_id: "synthetic-pulse".to_owned(),
        sources,
    })
}

async fn capture_profile(
    harness: SyncHarness,
    profile: PassthroughProfile,
    prefix: &str,
    candidate: bool,
) -> Result<PassthroughRun> {
    match profile {
        PassthroughProfile::Steady => harness.capture_passthrough(prefix).await,
        PassthroughProfile::Lifecycle => harness.capture_passthrough_lifecycle(prefix).await,
        PassthroughProfile::SharedWorker => {
            harness
                .capture_passthrough_shared_worker(prefix, candidate)
                .await
        }
    }
}

fn exact_pcm_matches(candidate: &[f32], control: &[f32]) -> bool {
    candidate.len() == control.len()
        && candidate
            .iter()
            .zip(control)
            .all(|(candidate, control)| candidate.to_bits() == control.to_bits())
}

fn evaluate_capture(candidate: &PcmCapture, control: &PcmCapture, failures: &mut Vec<String>) {
    if candidate.channels != control.channels || candidate.sample_rate != control.sample_rate {
        failures.push(format!(
            "{}: capture shape differs from control: candidate={}ch@{}Hz, control={}ch@{}Hz",
            candidate.label,
            candidate.channels,
            candidate.sample_rate,
            control.channels,
            control.sample_rate,
        ));
    }
    if candidate.samples.is_empty() || !candidate.samples.iter().any(|sample| *sample != 0.0) {
        failures.push(format!(
            "{}: passthrough capture has no audible PCM",
            candidate.label
        ));
    }
    if !candidate.backend_matches_tap {
        failures.push(format!(
            "{}: backend PCM differs from mix tap",
            candidate.label
        ));
    }
    if candidate.tap_dropped_samples != 0 {
        failures.push(format!(
            "{}: mix tap dropped {} samples",
            candidate.label, candidate.tap_dropped_samples,
        ));
    }
    if !control.backend_matches_tap {
        failures.push(format!(
            "{}: control backend PCM differs from mix tap",
            control.label
        ));
    }
    if control.tap_dropped_samples != 0 {
        failures.push(format!(
            "{}: control mix tap dropped {} samples",
            control.label, control.tap_dropped_samples,
        ));
    }
    if !exact_pcm_matches(&candidate.samples, &control.samples) {
        failures.push(format!(
            "{}: passthrough PCM differs bit-for-bit from the time-aligned control",
            candidate.label,
        ));
    }
    let candidate_report = CochleaReport::measure(
        &candidate.samples,
        candidate.channels,
        candidate.sample_rate,
    );
    let control_report =
        CochleaReport::measure(&control.samples, control.channels, control.sample_rate);
    failures.extend(continuity_failures(
        &candidate.label,
        &candidate_report,
        &control_report,
    ));
    if candidate_report.clipped_samples != 0 || candidate_report.true_peak_over_0dbtp {
        failures.push(format!(
            "{}: passthrough PCM clips (samples={}, over_0dbtp={})",
            candidate.label,
            candidate_report.clipped_samples,
            candidate_report.true_peak_over_0dbtp,
        ));
    }
}

fn evaluate(case: SyncCase, bundle: &PassthroughBundle) -> Vec<String> {
    let profile = match case.playback {
        PlaybackMode::Passthrough(profile) => profile,
        PlaybackMode::Sync => return vec![format!("{case}: expected passthrough playback")],
    };
    let mut failures = Vec::new();
    failures.extend(bundle.candidate.capture_failures.iter().cloned());
    failures.extend(
        bundle
            .control
            .capture_failures
            .iter()
            .map(|failure| format!("control: {failure}")),
    );
    for (run_name, run) in [
        ("candidate", &bundle.candidate),
        ("control", &bundle.control),
    ] {
        if (run.session_bpm - case.session_bpm).abs() > f64::EPSILON {
            failures.push(format!(
                "{run_name}: passthrough changed session BPM: expected {}, observed {}",
                case.session_bpm, run.session_bpm,
            ));
        }
        let expected_index = match profile {
            PassthroughProfile::Lifecycle => Some(1),
            PassthroughProfile::SharedWorker | PassthroughProfile::Steady => Some(0),
        };
        for (index, deck) in run.decks.iter().enumerate() {
            evaluate_deck(run_name, index, expected_index, deck, &mut failures);
        }
        match profile {
            PassthroughProfile::Lifecycle => {
                if run.reloads != case.decks {
                    failures.push(format!(
                        "{run_name}: lifecycle completed {} reloads, expected {}",
                        run.reloads, case.decks,
                    ));
                }
                if run.abr_switches != run.abr_switches_expected || run.abr_switch_failures != 0 {
                    failures.push(format!(
                        "{run_name}: lifecycle ABR applied={}, expected={}, failures={}",
                        run.abr_switches, run.abr_switches_expected, run.abr_switch_failures,
                    ));
                }
            }
            PassthroughProfile::SharedWorker | PassthroughProfile::Steady => {
                if run.reloads != 0 || run.abr_switches != 0 || run.abr_switch_failures != 0 {
                    failures.push(format!(
                        "{run_name}: steady passthrough unexpectedly ran lifecycle operations",
                    ));
                }
            }
        }
    }
    if profile == PassthroughProfile::SharedWorker && !bundle.candidate.load_observed {
        failures
            .push("candidate: shared-worker deadline load did not run during capture".to_owned());
    }
    if bundle.control.load_observed {
        failures.push("control: shared-worker load ran in the no-load control".to_owned());
    }
    evaluate_capture(&bundle.candidate.mix, &bundle.control.mix, &mut failures);
    if bundle.candidate.stems.len() != bundle.control.stems.len() {
        failures.push(format!(
            "passthrough stem count differs: candidate={}, control={}",
            bundle.candidate.stems.len(),
            bundle.control.stems.len(),
        ));
    }
    for (candidate, control) in bundle.candidate.stems.iter().zip(&bundle.control.stems) {
        evaluate_capture(candidate, control, &mut failures);
    }
    failures
}

fn evaluate_deck(
    run_name: &str,
    index: usize,
    expected_index: Option<usize>,
    deck: &PassthroughDeckOutcome,
    failures: &mut Vec<String>,
) {
    if deck.sync_index.is_some() {
        failures.push(format!(
            "{run_name} deck {index}: passthrough installed a SYNC binding"
        ));
    }
    if deck.rate.to_bits() != 1.0_f32.to_bits() {
        failures.push(format!(
            "{run_name} deck {index}: passthrough rate is {}, expected unity",
            deck.rate,
        ));
    }
    if !deck.playing {
        failures.push(format!(
            "{run_name} deck {index}: passthrough deck stopped playing"
        ));
    }
    if deck.current_index != expected_index {
        failures.push(format!(
            "{run_name} deck {index}: passthrough queue index is {:?}, expected {expected_index:?}",
            deck.current_index,
        ));
    }
    if deck.track_failed {
        failures.push(format!("{run_name} deck {index}: passthrough track failed"));
    }
    if deck.underruns != 0 {
        failures.push(format!(
            "{run_name} deck {index}: passthrough produced {} underruns",
            deck.underruns,
        ));
    }
    if deck.event_lagged != 0 || deck.event_stream_closed {
        failures.push(format!(
            "{run_name} deck {index}: event stream lost evidence (lagged={}, closed={})",
            deck.event_lagged, deck.event_stream_closed,
        ));
    }
}

fn persist(case: SyncCase, bundle: &PassthroughBundle, failures: &[String]) -> Result<()> {
    let profile = passthrough_profile(case)?;
    let mut metadata =
        SyncArtifactMetadata::new(case.id, case.sample_rate, CHANNELS, RENDER_FRAMES);
    metadata.set_operation(format!("passthrough-{}", profile.label()));
    metadata.add_state("media", bundle.media_id.clone());
    metadata.add_state("playback", "passthrough");
    metadata.add_state("profile", profile.label());
    metadata.add_state("rate", "unity");
    metadata.add_state("sync-bindings", "0");
    metadata.add_state(
        "candidate-underruns",
        bundle
            .candidate
            .decks
            .iter()
            .map(|deck| deck.underruns)
            .sum::<usize>()
            .to_string(),
    );
    metadata.add_state(
        "control-underruns",
        bundle
            .control
            .decks
            .iter()
            .map(|deck| deck.underruns)
            .sum::<usize>()
            .to_string(),
    );
    metadata.add_state(
        "shared-worker-load-observed",
        bundle.candidate.load_observed.to_string(),
    );
    let candidate_start = u64::try_from(bundle.candidate.mix.start_session_frame)
        .context("candidate capture start is non-negative")?;
    let control_start = u64::try_from(bundle.control.mix.start_session_frame)
        .context("control capture start is non-negative")?;
    metadata.add_frame(ArtifactFrame::new(
        candidate_start,
        "candidate-capture-start",
    ));
    metadata.add_frame(ArtifactFrame::new(control_start, "control-capture-start"));
    if let Some(seed) = bundle.library_seed {
        metadata.set_library_seed(seed);
    }
    for source in &bundle.sources {
        metadata.add_source(
            ArtifactSource::new(source.deck.clone(), source.media.clone())
                .with_analysis_key(source.analysis_key.clone()),
        );
    }
    metadata.add_failures(failures.iter().cloned());
    let audio = std::iter::once(&bundle.candidate.mix)
        .chain(std::iter::once(&bundle.control.mix))
        .chain(bundle.candidate.stems.iter())
        .chain(bundle.control.stems.iter())
        .map(|capture| ArtifactAudio::new(&capture.label, &capture.samples))
        .collect::<Vec<_>>();
    write_sync_artifact(&metadata, &audio).context("write optional passthrough artifact")?;
    Ok(())
}

fn assert_bundle(case: SyncCase, bundle: &PassthroughBundle) -> Result<()> {
    let failures = evaluate(case, bundle);
    persist(case, bundle, &failures)?;
    assert_rhythmic_oracle_load_bearing();
    if failures.is_empty() {
        Ok(())
    } else {
        bail!(
            "{case}: passthrough oracle rejected final PCM:\n{}",
            failures.join("\n"),
        )
    }
}

pub async fn assert_synthetic_passthrough_row(case: SyncCase) -> Result<()> {
    let bundle = run_synthetic_passthrough_row(case).await?;
    assert_bundle(case, &bundle)
}

pub async fn assert_passthrough_row(case: SyncCase, media: SyncMedia) -> Result<()> {
    let bundle = run_passthrough_row(case, media).await?;
    assert_bundle(case, &bundle)
}
