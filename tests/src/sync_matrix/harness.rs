use anyhow::{Context, Result, bail};
use kithara::{
    audio::{Beat, StretchControls, StretchKind, analysis::TrackAnalysis},
    bufpool::{BytePool, PcmPool},
    events::{AbrEvent, AudioEvent, DecoderEvent, Event, EventReceiver, TrackId},
    hls::AbrMode,
    platform::{
        sync::{Arc, Mutex},
        tokio::{sync::broadcast::error::TryRecvError, task::yield_now},
    },
    play::{Cmd, PlayerConfig, PlayerImpl, Reply, SessionDispatcher, SessionError, Tempo},
    queue::{Queue, QueueConfig, Transition},
};

use super::{
    CHANNELS, InitialDeckState, LedgerEntry, Operation, OperationOrder, RENDER_FRAMES, SyncCase,
    TempoRide,
    media::{SyncMedia, SyntheticFixture},
};
use crate::{
    offline::OfflineSession,
    sync_control::{SyncControlError, SyncDeckControl, SyncQuantum},
    sync_fixture::{analysis_map, unavailable_analysis_map},
};

const LOAD_PULL_LIMIT: usize = 4_000;
const SETTLE_PULL_LIMIT: usize = 4_000;
const ONSET_THRESHOLD: f32 = 0.015;

pub(super) struct SyncDeck {
    pub(super) abr_target: Option<usize>,
    abr_applied_target: Option<usize>,
    abr_wait_target: Option<usize>,
    pub(super) analysis: TrackAnalysis,
    pub(super) bpm: f64,
    pub(super) event_lagged: u64,
    pub(super) event_stream_closed: bool,
    pub(super) player: Arc<PlayerImpl>,
    pub(super) queue: Arc<Queue>,
    events: Mutex<EventReceiver>,
    reload_id: TrackId,
    pub(super) sync_index: Option<usize>,
    pub(super) underruns: usize,
}

pub(super) struct SyncHarness {
    pub(super) case: SyncCase,
    pub(super) abr_switch_failures: usize,
    pub(super) abr_switches: usize,
    pub(super) abr_switches_expected: usize,
    backend_capture: Option<Vec<f32>>,
    pub(super) capture_failures: Vec<String>,
    pub(super) decks: Vec<SyncDeck>,
    _fixture_guard: Option<SyntheticFixture>,
    pub(super) map_unavailable_errors: usize,
    pub(super) map_withdrawals: usize,
    pub(super) ledger: Vec<LedgerEntry>,
    pub(super) map_republishes: usize,
    pub(super) media: SyncMedia,
    pub(super) rendered_frames: u64,
    pub(super) reloads: usize,
    pub(super) rebinds: usize,
    pub(super) session: Arc<OfflineSession>,
    pub(super) tempo_ride_points: usize,
    pub(super) tempo_ride_requests: usize,
    pub(super) tempo_ride_transport_not_processed: usize,
}

impl SyncHarness {
    pub(super) async fn synthetic(case: SyncCase) -> Result<Self> {
        let fixture = SyntheticFixture::new(case)?;
        let media = fixture.media();
        Self::open_inner(case, media, Some(fixture)).await
    }

    pub(super) async fn open(case: SyncCase, media: SyncMedia) -> Result<Self> {
        Self::open_inner(case, media, None).await
    }

    async fn open_inner(
        case: SyncCase,
        media: SyncMedia,
        fixture_guard: Option<SyntheticFixture>,
    ) -> Result<Self> {
        if !(2..=4).contains(&case.decks) {
            bail!("{case}: behavioral matrix supports two to four decks");
        }
        media.validate(case)?;
        let session = Arc::new(OfflineSession::new_manual());
        let dispatcher = Arc::clone(&session) as Arc<dyn SessionDispatcher>;
        let mut decks = Vec::with_capacity(case.decks);

        for deck_index in 0..case.decks {
            let track = media.for_deck(deck_index);
            let controls = StretchControls::new(1.0);
            controls.set_backend(StretchKind::Signalsmith);
            controls.set_keylock(true);
            let player = Arc::new(PlayerImpl::new(
                PlayerConfig::builder()
                    .byte_pool(BytePool::default())
                    .pcm_pool(PcmPool::default())
                    .sample_rate(case.sample_rate)
                    .session(Arc::clone(&dispatcher))
                    .timestretch(controls)
                    .crossfade_duration(0.0)
                    .build(),
            ));
            player
                .ensure_engine_started()
                .with_context(|| format!("{case}: start offline player engine"))?;
            let queue = Arc::new(Queue::new(
                QueueConfig::builder().player(Arc::clone(&player)).build(),
            ));
            let _ = queue.append(track.source.clone());
            let reload_id = queue.append(track.source.clone());
            decks.push(SyncDeck {
                abr_target: track.abr_target,
                abr_applied_target: None,
                abr_wait_target: None,
                analysis: track.analysis.clone(),
                bpm: track
                    .bpm()
                    .with_context(|| format!("{case}: deck {deck_index} has no beat grid"))?,
                event_lagged: 0,
                event_stream_closed: false,
                events: Mutex::new(player.subscribe()),
                player,
                queue,
                reload_id,
                sync_index: None,
                underruns: 0,
            });
        }

        let mut harness = Self {
            abr_switch_failures: 0,
            abr_switches: 0,
            abr_switches_expected: media
                .tracks
                .iter()
                .cycle()
                .take(case.decks)
                .filter(|track| track.abr_target.is_some())
                .count(),
            backend_capture: None,
            case,
            capture_failures: Vec::new(),
            decks,
            _fixture_guard: fixture_guard,
            map_unavailable_errors: 0,
            map_withdrawals: 0,
            ledger: Vec::new(),
            map_republishes: 0,
            media,
            rendered_frames: 0,
            reloads: 0,
            rebinds: 0,
            session,
            tempo_ride_points: 0,
            tempo_ride_requests: 0,
            tempo_ride_transport_not_processed: 0,
        };
        harness.establish_grid(true).await?;
        harness.load_paused().await?;
        Ok(harness)
    }

    #[must_use]
    pub(super) fn deck_gain(&self) -> f32 {
        (1.0 / self.decks.len() as f32).min(0.25)
    }

    pub(super) fn start_backend_capture(&mut self) -> Result<()> {
        if self.backend_capture.is_some() {
            bail!("{}: backend PCM capture is already active", self.case);
        }
        self.backend_capture = Some(Vec::new());
        Ok(())
    }

    pub(super) fn finish_backend_capture(&mut self) -> Result<Vec<f32>> {
        self.backend_capture
            .take()
            .with_context(|| format!("{}: backend PCM capture is not active", self.case))
    }

    async fn load_paused(&mut self) -> Result<()> {
        for deck in &self.decks {
            deck.queue.play();
        }
        self.record("load-play-all");
        let mut heard_audio = false;
        for _ in 0..LOAD_PULL_LIMIT {
            let block = self.render_block(RENDER_FRAMES)?;
            heard_audio |= block.iter().any(|sample| sample.abs() > ONSET_THRESHOLD);
            if heard_audio && self.decks.iter().all(|deck| deck.player.is_playing()) {
                break;
            }
            yield_now().await;
        }
        if !heard_audio || self.decks.iter().any(|deck| !deck.player.is_playing()) {
            bail!("{}: synthetic WAVs did not all produce PCM", self.case);
        }
        for deck in &self.decks {
            deck.queue.pause();
        }
        self.record("load-pause-all");
        self.render_frames(RENDER_FRAMES * 2).await?;
        self.reset_paused_positions().await
    }

    async fn reset_paused_positions(&mut self) -> Result<()> {
        for deck in &self.decks {
            deck.queue
                .seek(0.0)
                .with_context(|| format!("{}: reset paused deck to source start", self.case))?;
        }
        self.record("load-reset-all");
        for _ in 0..SETTLE_PULL_LIMIT {
            self.render_frames(RENDER_FRAMES).await?;
            let reset = self.decks.iter().all(|deck| {
                deck.queue
                    .position_seconds()
                    .is_some_and(|position| position <= 0.05)
            });
            if reset {
                return Ok(());
            }
        }
        bail!("{}: paused decks did not reset to source start", self.case)
    }

    async fn establish_grid(&mut self, playing: bool) -> Result<()> {
        self.set_session_tempo(self.case.session_bpm).await?;
        self.set_session_playing(playing).await
    }

    async fn set_session_tempo(&mut self, bpm: f64) -> Result<()> {
        self.commit_session_tempo(bpm, RENDER_FRAMES).await
    }

    async fn commit_session_tempo(&mut self, bpm: f64, render_frames: usize) -> Result<()> {
        let tempo = Tempo::new(bpm).context("valid session tempo")?;
        self.exec_ok(Cmd::SetSessionTempo { tempo }, "set session tempo")
            .with_context(|| {
                format!(
                    "{}: commit {bpm:.6} BPM at rendered frame {}",
                    self.case, self.rendered_frames,
                )
            })?;
        self.record("session-tempo");
        let _ = self.render_frames(render_frames).await?;
        Ok(())
    }

    async fn set_session_playing(&mut self, playing: bool) -> Result<()> {
        self.exec_ok(
            Cmd::SetSessionPlaying { playing },
            "set session transport playing",
        )?;
        self.record(if playing {
            "session-start"
        } else {
            "session-stop"
        });
        let _ = self.render_frames(RENDER_FRAMES).await?;
        Ok(())
    }

    fn exec_ok(&self, cmd: Cmd, action: &str) -> Result<()> {
        match self.session.exec(cmd) {
            Ok(Reply::Ok) => Ok(()),
            Ok(Reply::Err(error)) => bail!("{}: {action} failed: {error}", self.case),
            Ok(_) => bail!("{}: {action} returned an unexpected reply", self.case),
            Err(error) => bail!(
                "{}: {action} could not reach the session: {error}",
                self.case
            ),
        }
    }

    pub(super) async fn start_initial(&mut self) -> Result<()> {
        match self.case.initial {
            InitialDeckState::Paused => Ok(()),
            InitialDeckState::RunningStaggered => self.start_staggered().await,
        }
    }

    pub(super) async fn start_staggered(&mut self) -> Result<()> {
        for deck_index in 0..self.decks.len() {
            self.decks[deck_index].queue.play();
            self.record(if deck_index == 0 {
                "deck-0-play"
            } else {
                "follower-play"
            });
            if deck_index + 1 < self.decks.len() {
                let follower = deck_index + 1;
                let delay = self.analyzed_stagger_frames(follower)?;
                self.render_frames(delay).await?;
            }
        }
        self.wait_all_playing().await
    }

    fn analyzed_stagger_frames(&self, deck_index: usize) -> Result<usize> {
        let analysis = &self
            .decks
            .get(deck_index)
            .with_context(|| format!("{}: no stagger source deck {deck_index}", self.case))?
            .analysis;
        let beats = analysis
            .beat()
            .context("stagger source analysis has no beat grid")?
            .beats();
        let interval = beats
            .windows(2)
            .find_map(|window| window[1].checked_sub(window[0]).filter(|span| *span > 0))
            .context("stagger source beat grid has no positive local interval")?;
        let source_rate = analysis
            .source_sample_rate()
            .context("stagger source analysis has no source sample-rate axis")?;
        Ok((interval as f64 / f64::from(source_rate.get())
            * f64::from(self.case.sample_rate)
            * self.case.stagger_beats)
            .round() as usize)
    }

    pub(super) async fn apply_order(&mut self) -> Result<()> {
        if self.case.order == OperationOrder::SequentialSync {
            return self.apply_sequential_sync().await;
        }
        for (stage, &operation) in self.case.order.operations().iter().enumerate() {
            for offset in 0..self.decks.len() {
                let deck_index = (offset + stage) % self.decks.len();
                self.apply(deck_index, operation)?;
                self.render_frames(RENDER_FRAMES * 2).await?;
            }
        }
        let _ = self.render_frames(RENDER_FRAMES * 8).await?;
        Ok(())
    }

    pub(super) async fn apply_free_order(&mut self) -> Result<()> {
        if self.case.order == OperationOrder::SequentialSync {
            for index in 0..self.decks.len() {
                self.apply_free(index, Operation::Play)?;
            }
            self.wait_all_playing().await?;
            for index in 0..self.decks.len() {
                self.apply_free(index, Operation::Sync)?;
                let _ = self.render_frames(RENDER_FRAMES * 4).await?;
            }
            for index in 0..self.decks.len() {
                self.apply_free(index, Operation::Seek)?;
            }
            let _ = self.render_frames(RENDER_FRAMES * 8).await?;
            return Ok(());
        }
        for (stage, &operation) in self.case.order.operations().iter().enumerate() {
            for offset in 0..self.decks.len() {
                let deck_index = (offset + stage) % self.decks.len();
                self.apply_free(deck_index, operation)?;
                self.render_frames(RENDER_FRAMES * 2).await?;
            }
        }
        let _ = self.render_frames(RENDER_FRAMES * 8).await?;
        Ok(())
    }

    async fn apply_sequential_sync(&mut self) -> Result<()> {
        for index in 0..self.decks.len() {
            self.apply(index, Operation::Play)?;
        }
        self.wait_all_playing().await?;
        for index in 0..self.decks.len() {
            self.apply(index, Operation::Sync)?;
            let _ = self.render_frames(RENDER_FRAMES * 4).await?;
        }
        for index in 0..self.decks.len() {
            self.apply(index, Operation::Seek)?;
        }
        let _ = self.render_frames(RENDER_FRAMES * 8).await?;
        Ok(())
    }

    pub(super) fn apply(&mut self, deck_index: usize, operation: Operation) -> Result<()> {
        let Some(deck) = self.decks.get_mut(deck_index) else {
            bail!("{}: no deck {deck_index}", self.case);
        };
        match operation {
            Operation::Play => deck.queue.play(),
            Operation::Seek => {
                deck.queue
                    .seek(self.case.seek_seconds)
                    .with_context(|| format!("{}: deck {deck_index} seek", self.case))?;
            }
            Operation::Sync => {
                let index = deck.queue.current_index().with_context(|| {
                    format!(
                        "{}: deck {deck_index} has no current queue index",
                        self.case
                    )
                })?;
                deck.sync_index = Some(index);
                let quantum = SyncQuantum::new(4.0).context("valid bar quantum")?;
                let map = analysis_map(&deck.analysis, "behavioral deck")?;
                deck.queue
                    .start_at_map(index, map, Beat::default(), quantum)
                    .with_context(|| format!("{}: deck {deck_index} sync", self.case))?;
            }
        }
        self.record(operation.label());
        Ok(())
    }

    fn apply_free(&mut self, deck_index: usize, operation: Operation) -> Result<()> {
        let Some(deck) = self.decks.get(deck_index) else {
            bail!("{}: no free-control deck {deck_index}", self.case);
        };
        match operation {
            Operation::Play => deck.queue.play(),
            Operation::Seek => {
                deck.queue.seek(self.case.seek_seconds).with_context(|| {
                    format!("{}: free-control deck {deck_index} seek", self.case)
                })?;
            }
            Operation::Sync => {}
        }
        self.record(match operation {
            Operation::Play => "control-play",
            Operation::Seek => "control-seek",
            Operation::Sync => "control-sync-slot",
        });
        Ok(())
    }

    pub(super) async fn ride_tempo(&mut self, ride: TempoRide) -> Result<()> {
        let (updates_hz, updates_per_leg) = self.tempo_ride_cadence()?;
        let ride_start = self.rendered_frames;
        let mut request_count = 0_u64;
        let mut start = self.case.session_bpm;
        for &target in ride.points() {
            for update in 1..=updates_per_leg {
                let fraction = f64::from(update) / f64::from(updates_per_leg);
                let bpm = (target - start).mul_add(fraction, start);
                let _ = self.attempt_ride_tempo(bpm)?;
                request_count = request_count.saturating_add(1);
                self.render_to_ride_deadline(ride_start, request_count, updates_hz)
                    .await?;
            }
            start = target;
        }
        Ok(())
    }

    pub(super) async fn ride_free_tempo(&mut self, ride: TempoRide) -> Result<()> {
        let (updates_hz, updates_per_leg) = self.tempo_ride_cadence()?;
        let ride_start = self.rendered_frames;
        let mut request_count = 0_u64;
        let mut start = self.case.session_bpm;
        for &target in ride.points() {
            for update in 1..=updates_per_leg {
                let fraction = f64::from(update) / f64::from(updates_per_leg);
                let bpm = (target - start).mul_add(fraction, start);
                if self.attempt_ride_tempo(bpm)? {
                    for deck in &self.decks {
                        deck.queue.set_rate((bpm / deck.bpm) as f32);
                    }
                }
                request_count = request_count.saturating_add(1);
                self.render_to_ride_deadline(ride_start, request_count, updates_hz)
                    .await?;
            }
            start = target;
        }
        Ok(())
    }

    fn tempo_ride_cadence(&self) -> Result<(u32, u32)> {
        let updates_hz = self.case.tempo_updates_hz;
        if updates_hz < 2 || !updates_hz.is_multiple_of(2) {
            bail!(
                "{}: tempo update rate must be an even value of at least 2 Hz, got {updates_hz}",
                self.case
            );
        }
        if updates_hz > self.case.sample_rate {
            bail!(
                "{}: tempo update rate {updates_hz} exceeds sample rate {}",
                self.case,
                self.case.sample_rate
            );
        }
        Ok((updates_hz, updates_hz / 2))
    }

    fn attempt_ride_tempo(&mut self, bpm: f64) -> Result<bool> {
        let tempo = Tempo::new(bpm).context("valid session tempo")?;
        self.tempo_ride_requests = self.tempo_ride_requests.saturating_add(1);
        self.record("tempo-ride-request");
        match self.session.exec(Cmd::SetSessionTempo { tempo }) {
            Ok(Reply::Ok) => {
                self.tempo_ride_points = self.tempo_ride_points.saturating_add(1);
                self.record("tempo-ride-accepted");
                Ok(true)
            }
            Ok(Reply::Err(SessionError::TransportNotProcessed)) => {
                self.tempo_ride_transport_not_processed =
                    self.tempo_ride_transport_not_processed.saturating_add(1);
                self.record("tempo-ride-rejected-transport-not-processed");
                Ok(false)
            }
            Ok(Reply::Err(error)) => {
                bail!("{}: tempo ride at {bpm:.6} BPM failed: {error}", self.case)
            }
            Ok(_) => bail!("{}: tempo ride returned an unexpected reply", self.case),
            Err(error) => bail!(
                "{}: tempo ride could not reach the session: {error}",
                self.case
            ),
        }
    }

    async fn render_to_ride_deadline(
        &mut self,
        ride_start: u64,
        request_count: u64,
        updates_hz: u32,
    ) -> Result<()> {
        let elapsed = request_count
            .checked_mul(u64::from(self.case.sample_rate))
            .context("tempo ride frame deadline overflow")?
            / u64::from(updates_hz);
        let deadline = ride_start
            .checked_add(elapsed)
            .context("tempo ride session-frame deadline overflow")?;
        let frames = deadline
            .checked_sub(self.rendered_frames)
            .with_context(|| {
                format!(
                    "{}: tempo ride deadline regressed from {} to {deadline}",
                    self.case, self.rendered_frames
                )
            })?;
        let frames = usize::try_from(frames).context("tempo ride interval fits usize")?;
        let _ = self.render_frames(frames).await?;
        Ok(())
    }

    fn republish_analysis(&mut self, deck_index: usize, analysis: &TrackAnalysis) -> Result<()> {
        let deck = self
            .decks
            .get_mut(deck_index)
            .with_context(|| format!("{}: no deck {deck_index}", self.case))?;
        let quantum = SyncQuantum::new(4.0).context("valid bar quantum")?;
        let index = deck.queue.current_index().with_context(|| {
            format!(
                "{}: deck {deck_index} has no current queue index",
                self.case
            )
        })?;
        deck.sync_index = Some(index);
        let map = analysis_map(analysis, "republished behavioral deck")?;
        deck.queue
            .start_at_map(index, map, Beat::default(), quantum)
            .with_context(|| format!("{}: republish deck {deck_index} map", self.case))?;
        self.map_republishes = self.map_republishes.saturating_add(1);
        self.record("map-republish");
        Ok(())
    }

    fn withdraw_analysis(&mut self, deck_index: usize) -> Result<()> {
        let deck = self
            .decks
            .get(deck_index)
            .with_context(|| format!("{}: no deck {deck_index}", self.case))?;
        deck.queue.unbind_from_map().with_context(|| {
            format!(
                "{}: unbind deck {deck_index} before map withdrawal",
                self.case
            )
        })?;
        let quantum = SyncQuantum::new(4.0).context("valid bar quantum")?;
        let index = deck.queue.current_index().with_context(|| {
            format!(
                "{}: deck {deck_index} has no current queue index",
                self.case
            )
        })?;
        let map = unavailable_analysis_map(&deck.analysis, "withdrawn behavioral deck")?;
        let unavailable = match deck
            .queue
            .start_at_map(index, map, Beat::default(), quantum)
        {
            Ok(()) => false,
            Err(SyncControlError::MapUnavailable { .. }) => true,
            Err(error) => return Err(error.into()),
        };
        if unavailable {
            self.map_unavailable_errors = self.map_unavailable_errors.saturating_add(1);
        }
        self.map_withdrawals = self.map_withdrawals.saturating_add(1);
        self.record("map-unavailable");
        Ok(())
    }

    pub(super) async fn exercise_map_refinement(&mut self) -> Result<()> {
        for deck_index in 0..self.decks.len() {
            self.withdraw_analysis(deck_index)?;
            self.render_frames(RENDER_FRAMES * 2).await?;
            let full = self.decks[deck_index].analysis.clone();
            let partial = partial_analysis(&full)?;
            self.republish_analysis(deck_index, &partial)?;
            self.render_frames(RENDER_FRAMES * 2).await?;
            self.republish_analysis(deck_index, &full)?;
            self.render_frames(RENDER_FRAMES * 2).await?;
        }
        Ok(())
    }

    pub(super) async fn reload_all(&mut self) -> Result<()> {
        for deck_index in 0..self.decks.len() {
            let reload_id = self.decks[deck_index].reload_id;
            let reload_index = self.decks[deck_index]
                .queue
                .tracks()
                .iter()
                .position(|track| track.id == reload_id)
                .with_context(|| {
                    format!("{}: reload track is not in deck {deck_index}", self.case)
                })?;
            self.decks[deck_index]
                .queue
                .select(reload_id, Transition::None)
                .with_context(|| format!("{}: reload deck {deck_index}", self.case))?;
            self.reloads = self.reloads.saturating_add(1);
            self.record("reload");
            self.wait_deck_index(deck_index, reload_index).await?;
            let analysis = self.decks[deck_index].analysis.clone();
            self.republish_analysis(deck_index, &analysis)?;
        }
        self.render_frames(RENDER_FRAMES * 8).await?;
        Ok(())
    }

    pub(super) async fn reload_all_free(&mut self) -> Result<()> {
        let final_bpm = self.case.tempo_ride.final_bpm();
        for deck_index in 0..self.decks.len() {
            let reload_id = self.decks[deck_index].reload_id;
            let reload_index = self.decks[deck_index]
                .queue
                .tracks()
                .iter()
                .position(|track| track.id == reload_id)
                .with_context(|| {
                    format!(
                        "{}: free-control reload track is not in deck {deck_index}",
                        self.case
                    )
                })?;
            self.decks[deck_index]
                .queue
                .select(reload_id, Transition::None)
                .with_context(|| format!("{}: reload free-control deck {deck_index}", self.case))?;
            self.reloads = self.reloads.saturating_add(1);
            self.record("control-reload");
            self.wait_deck_index(deck_index, reload_index).await?;
            let rate = (final_bpm / self.decks[deck_index].bpm) as f32;
            self.decks[deck_index].queue.set_rate(rate);
        }
        self.render_frames(RENDER_FRAMES * 8).await?;
        Ok(())
    }

    async fn wait_deck_index(&mut self, deck_index: usize, expected: usize) -> Result<()> {
        for _ in 0..SETTLE_PULL_LIMIT {
            let landed = self.decks.get(deck_index).is_some_and(|deck| {
                deck.queue.current_index() == Some(expected) && deck.player.is_playing()
            });
            if landed {
                self.record("reload-index-landed");
                return Ok(());
            }
            self.render_frames(RENDER_FRAMES).await?;
        }
        let observed = self
            .decks
            .get(deck_index)
            .and_then(|deck| deck.queue.current_index());
        bail!(
            "{}: deck {deck_index} reload did not land at queue index {expected}; observed={observed:?}",
            self.case
        )
    }

    pub(super) async fn switch_abr_variants(&mut self) -> Result<()> {
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
            self.record("abr-switch");
            for _ in 0..SETTLE_PULL_LIMIT {
                self.render_frames(RENDER_FRAMES).await?;
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

    async fn wait_all_playing(&mut self) -> Result<()> {
        for _ in 0..SETTLE_PULL_LIMIT {
            if self.decks.iter().all(|deck| deck.player.is_playing()) {
                return Ok(());
            }
            self.render_frames(RENDER_FRAMES).await?;
        }
        bail!("{}: decks did not all reach playing state", self.case)
    }

    pub(super) async fn render_frames(&mut self, frames: usize) -> Result<Vec<f32>> {
        let mut rendered = Vec::with_capacity(frames.saturating_mul(usize::from(CHANNELS)));
        let mut remaining = frames;
        while remaining > 0 {
            let block_frames = remaining.min(RENDER_FRAMES);
            rendered.extend(self.render_block(block_frames)?);
            remaining -= block_frames;
            yield_now().await;
        }
        Ok(rendered)
    }

    fn render_block(&mut self, frames: usize) -> Result<Vec<f32>> {
        let block = self.session.render(frames);
        if let Some(capture) = &mut self.backend_capture {
            capture.extend_from_slice(&block);
        }
        self.rendered_frames = self.rendered_frames.saturating_add(frames as u64);
        self.tick()?;
        Ok(block)
    }

    fn tick(&mut self) -> Result<()> {
        let capture_active = self.backend_capture.is_some();
        let mut capture_failures = Vec::new();
        for (deck_index, deck) in self.decks.iter_mut().enumerate() {
            if let Err(error) = deck.queue.tick() {
                let failure = format!("{}: tick deck {deck_index}: {error}", self.case);
                if capture_active {
                    capture_failures.push(failure);
                } else {
                    bail!(failure);
                }
            }
            let event_failures = drain_deck_events(deck, deck_index, self.case, capture_active);
            if capture_active {
                capture_failures.extend(event_failures);
            } else if let Some(failure) = event_failures.into_iter().next() {
                bail!(failure);
            }
        }
        self.capture_failures.extend(capture_failures);
        Ok(())
    }

    pub(super) fn record(&mut self, event: &'static str) {
        self.ledger.push(LedgerEntry {
            event,
            frame: self.rendered_frames,
        });
    }
}

fn partial_analysis(analysis: &TrackAnalysis) -> Result<TrackAnalysis> {
    let grid = analysis.beat().context("analysis has no beat grid")?;
    let keep = (grid.beats().len() / 2).max(8).min(grid.beats().len());
    let beats = grid.beats()[..keep].to_vec();
    let last = beats
        .last()
        .copied()
        .context("analysis beat grid is empty")?;
    let downbeats = grid
        .downbeats()
        .iter()
        .copied()
        .take_while(|frame| *frame <= last)
        .collect();
    let rate = analysis
        .source_sample_rate()
        .context("analysis has no source sample rate")?;
    Ok(TrackAnalysis::with_source_rate(
        Some(kithara::audio::BeatGrid::new(
            grid.bpm(),
            beats,
            downbeats,
            Vec::new(),
        )),
        analysis.waveform().cloned(),
        analysis.source_frames(),
        rate,
    ))
}

fn drain_deck_events(
    deck: &mut SyncDeck,
    deck_index: usize,
    case: SyncCase,
    capture_active: bool,
) -> Vec<String> {
    let mut underruns = 0_usize;
    let mut lagged = 0_u64;
    let mut stream_closed = false;
    let mut applied_target = None;
    let mut failures = Vec::new();
    {
        let mut events = deck.events.lock();
        loop {
            match events.try_recv().map(|envelope| envelope.event) {
                Ok(Event::Decoder(DecoderEvent::DecodeError { kind, detail, .. })) => {
                    failures.push(format!(
                        "{case}: deck {deck_index} decoder failed: {kind:?}: {detail}"
                    ));
                }
                Ok(Event::Audio(AudioEvent::UnderrunStarted { .. })) => {
                    if capture_active {
                        underruns = underruns.saturating_add(1);
                    }
                }
                Ok(Event::Abr(AbrEvent::VariantApplied { to, .. })) => {
                    if deck.abr_wait_target == Some(to.get()) {
                        applied_target = Some(to.get());
                    }
                }
                Ok(_) => {}
                Err(TryRecvError::Lagged(skipped)) => {
                    lagged = lagged.saturating_add(skipped);
                }
                Err(TryRecvError::Closed) => {
                    stream_closed = true;
                    break;
                }
                Err(TryRecvError::Empty) => break,
            }
        }
    }
    deck.event_lagged = deck.event_lagged.saturating_add(lagged);
    deck.event_stream_closed |= stream_closed;
    deck.underruns = deck.underruns.saturating_add(underruns);
    if applied_target.is_some() {
        deck.abr_applied_target = applied_target;
    }
    failures
}
