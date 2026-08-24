use std::{f64::consts::TAU, fs, mem::size_of, path::Path};

use ::kithara::{
    assets::{AssetStore, StorageBackend},
    audio::analysis::TrackAnalysis,
    bufpool::{BytePool, PcmPool},
    net::{HttpClient, NetOptions},
    play::{Cmd, Reply, SessionDispatcher, SessionHandle, SessionTransportSnapshot},
    stream::dl::{Downloader, DownloaderConfig},
};
use cochlea_features::{Audio, ProbeOpts, TempoOpts, TempoReport, estimate_tempo, probe};
use iced::window;
use kithara_platform::{
    CancelScope,
    sync::Arc,
    time::{self, Duration},
    tokio::task::yield_now,
};
use kithara_queue::{Queue, TrackId, TrackStatus, Transition};
use kithara_test_utils::kithara;
use kithara_ui::render::{ControlAction, UiEvent};
use num_traits::{ToPrimitive, cast::AsPrimitive};
use tempfile::TempDir;

use self::{
    artifact::write_optional_artifacts,
    fixture::{
        A, A_BPM, B, B_BPM, B_UNUSED_SECONDS, CAPTURE_SECONDS, LOAD_YIELD_LIMIT,
        PHASE_BUDGET_FRAMES, STAGGER_BEATS, SYNC_SETTLE_PULL_LIMIT, TRACK_SECONDS, WARM_PULL_LIMIT,
    },
    offline::{BLOCK_FRAMES, CHANNELS, OfflineSession, SAMPLE_RATE},
};
use super::{
    app::{Decks, Kithara},
    message::Message,
    ui::AppUi,
    update,
};
use crate::{
    broadcast::Broadcaster,
    catalog::Catalog,
    config::AppConfig,
    deck::{Deck, DeckId, DeckSet, EqMode},
    state::StateController,
};

mod artifact;
mod fixture {
    use super::DeckId;

    pub(super) const A: DeckId = DeckId(0);
    pub(super) const A_BPM: f64 = 120.0;
    pub(super) const ARTIFACT_CASE: &str = "app-raw-ui-sync";
    pub(super) const ARTIFACT_DIR_ENV: &str = "KITHARA_SYNC_ARTIFACT_DIR";
    pub(super) const B: DeckId = DeckId(1);
    pub(super) const B_BPM: f64 = 126.0;
    pub(super) const B_UNUSED_SECONDS: usize = 12;
    pub(super) const CAPTURE_SECONDS: usize = 4;
    pub(super) const LOAD_YIELD_LIMIT: usize = 20_000;
    pub(super) const PHASE_BUDGET_FRAMES: u64 = 512;
    pub(super) const STAGGER_BEATS: f64 = 3.0 / 8.0;
    pub(super) const SYNC_SETTLE_PULL_LIMIT: usize = 512;
    pub(super) const TRACK_SECONDS: usize = 30;
    pub(super) const WARM_PULL_LIMIT: usize = 4_000;
}

mod offline;

#[kithara::test(
    native,
    tokio,
    multi_thread,
    flash(false),
    timeout(Duration::from_secs(180))
)]
#[ignore = "SYNC-ORACLE SYNC-PRODUCT-UI-001: waiting for Wave AppToggle"]
async fn raw_sync_controls_adopt_one_grid_and_bind_the_actual_tracks() {
    let mut trace = AppTrace::new().await;
    let a_selected = trace
        .wait_for_selected_analysis(A, trace.a_track, 0, TRACK_SECONDS)
        .await;
    let b_selected = trace
        .wait_for_selected_analysis(B, trace.b_track, 1, TRACK_SECONDS)
        .await;
    let expected = SyncExpectation::from((&a_selected.analysis, &b_selected.analysis));

    let control_warm_pcm = trace.warm_and_reset().await;
    trace.start_staggered(expected.stagger_frames).await;
    let control_a_capture = trace.capture_deck(A).await;
    let control_b_capture = trace.capture_deck(B).await;
    let control_mix_capture = trace.capture_mix().await;

    let sync_warm_pcm = trace.warm_and_reset().await;
    trace.start_staggered(expected.stagger_frames).await;

    let a_position = trace
        .queue_a
        .position_seconds()
        .expect("invariant: warm decks expose the primary source position");
    let b_position = trace
        .queue_b
        .position_seconds()
        .expect("invariant: warm decks expose the secondary source position");
    let stagger_phase = phase_distance(
        track_beat_at(&a_selected.analysis, a_position),
        track_beat_at(&b_selected.analysis, b_position),
    );
    let transport_initially_absent = trace.transport().is_err();

    trace.press_sync(A);
    let pending_after_primary_press = trace.pending_sync();
    let primary_light_after_press = trace.sync_light(A);

    let primary_sync_visible = trace.wait_for_sync_mode(A).await;
    let primary = trace.transport().ok();
    let primary_revision = primary.as_ref().map(SessionTransportSnapshot::revision);

    trace.press_sync(B);
    let pending_after_secondary_press = trace.pending_sync();
    let b_sync_light = trace.sync_light(B);
    let secondary_sync_visible = trace.wait_for_sync_mode(B).await;
    let adopted = trace.transport().ok();

    let a_capture = trace.capture_deck(A).await;
    let b_capture = trace.capture_deck(B).await;
    let mix_capture = trace.capture_mix().await;
    let captures = SyncCaptures {
        unsynced_deck_a: control_a_capture,
        unsynced_deck_b: control_b_capture,
        unsynced_mix: control_mix_capture,
        synced_deck_a: a_capture,
        synced_deck_b: b_capture,
        synced_mix: mix_capture,
    };

    trace.press_sync(B);
    let pending_after_secondary_disable = trace.pending_sync();
    let b_light_after_disable = trace.sync_light(B);
    let _ = trace.render_frames(BLOCK_FRAMES).await;
    let a_light_after_secondary_disable = trace.sync_light(A);
    let b_light_after_disable_render = trace.sync_light(B);

    write_optional_artifacts(&captures, &expected, &a_selected, &b_selected);

    assert_eq!(a_selected.index, 0);
    assert_eq!(a_selected.track_id, trace.a_track);
    assert_eq!(b_selected.index, 1);
    assert_eq!(b_selected.track_id, trace.b_track);
    assert_eq!(trace.queue_a.current_index(), Some(a_selected.index));
    assert_eq!(trace.queue_b.current_index(), Some(b_selected.index));
    assert!(
        control_warm_pcm && sync_warm_pcm,
        "real Queue/Player path must render fixture PCM before both captures"
    );
    assert!(
        stagger_phase.is_some_and(|phase| phase > 0.25),
        "setup must start the analysed decks off-grid, got {stagger_phase:?} beats"
    );
    assert!(
        transport_initially_absent,
        "the GUI must exercise the pending first-grid path"
    );
    assert_eq!(pending_after_primary_press, Some(A));
    assert!(primary_light_after_press);
    assert!(
        primary_sync_visible,
        "primary SYNC never settled as enabled in app-visible state"
    );
    let primary = primary.expect("the first SYNC press must install the session grid");
    assert_eq!(primary.tempo().beats_per_minute(), expected.primary_bpm);
    assert_eq!(pending_after_secondary_press, None);

    let unsynced_a_report = cochlea_report(&captures.unsynced_deck_a, "unsynced deck A");
    let unsynced_b_report = cochlea_report(&captures.unsynced_deck_b, "unsynced deck B");
    assert_cochlea_clean(&captures.unsynced_mix, "two-deck unsynced mix");
    let synced_a_report = cochlea_report(&captures.synced_deck_a, "synced deck A");
    let synced_b_report = cochlea_report(&captures.synced_deck_b, "synced deck B");
    let _mix_report = cochlea_report(&captures.synced_mix, "two-deck synced mix");

    assert!(b_sync_light, "secondary SYNC light did not stay enabled");
    assert!(
        secondary_sync_visible,
        "secondary SYNC never settled as enabled in app-visible state"
    );
    let adopted = adopted.expect("secondary deck must reuse the primary grid");
    assert_eq!(adopted.tempo().beats_per_minute(), expected.primary_bpm);
    assert_eq!(Some(adopted.revision()), primary_revision);
    assert_eq!(trace.queue_a.current_index(), Some(0));
    assert_eq!(
        trace.queue_a.current().map(|track| track.id),
        Some(trace.a_track)
    );
    assert_eq!(trace.queue_b.current_index(), Some(1));
    assert_eq!(
        trace.queue_b.current().map(|track| track.id),
        Some(trace.b_track)
    );
    assert_eq!(pending_after_secondary_disable, None);
    assert!(!b_light_after_disable);
    assert!(a_light_after_secondary_disable);
    assert!(!b_light_after_disable_render);

    assert!(
        (synced_a_report.bpm - expected.primary_bpm).abs() <= 1.0
            && (synced_b_report.bpm - expected.primary_bpm).abs() <= 1.0,
        "post-SYNC Cochlea tempo must follow the analysed primary grid at {:.3} BPM: A={:.3}, B={:.3}",
        expected.primary_bpm,
        synced_a_report.bpm,
        synced_b_report.bpm
    );
    let beat_period = (f64::from(SAMPLE_RATE) * 60.0 / expected.primary_bpm)
        .round()
        .to_u64()
        .expect("fixture beat period fits u64");
    let (unsynced_a_phase, unsynced_a_concentration) =
        circular_phase(&unsynced_a_report.beat_frames, beat_period)
            .expect("unsynced deck A Cochlea beats must have a phase");
    let (unsynced_b_phase, unsynced_b_concentration) =
        circular_phase(&unsynced_b_report.beat_frames, beat_period)
            .expect("unsynced deck B Cochlea beats must have a phase");
    assert!(
        unsynced_a_concentration >= 0.5 && unsynced_b_concentration >= 0.5,
        "unsynced Cochlea phase must be stable: A={unsynced_a_concentration:.3}, B={unsynced_b_concentration:.3}"
    );
    let unsynced_spread = circular_spread(&[unsynced_a_phase, unsynced_b_phase], beat_period)
        .expect("two unsynced deck phases must produce a spread");
    assert!(
        unsynced_spread > PHASE_BUDGET_FRAMES,
        "pre-SYNC Cochlea beat spread is {unsynced_spread} frames; it must exceed the {PHASE_BUDGET_FRAMES}-frame post-SYNC budget"
    );

    let (synced_a_phase, synced_a_concentration) =
        circular_phase(&synced_a_report.beat_frames, beat_period)
            .expect("synced deck A Cochlea beats must have a phase");
    let (synced_b_phase, synced_b_concentration) =
        circular_phase(&synced_b_report.beat_frames, beat_period)
            .expect("synced deck B Cochlea beats must have a phase");
    assert!(
        synced_a_concentration >= 0.5 && synced_b_concentration >= 0.5,
        "synced Cochlea phase must be stable: A={synced_a_concentration:.3}, B={synced_b_concentration:.3}"
    );
    let synced_spread = circular_spread(&[synced_a_phase, synced_b_phase], beat_period)
        .expect("two synced deck phases must produce a spread");
    assert!(
        synced_spread <= PHASE_BUDGET_FRAMES,
        "post-SYNC Cochlea beat spread is {synced_spread} frames; budget is {PHASE_BUDGET_FRAMES}"
    );
}

struct AppTrace {
    _temp: TempDir,
    app: Kithara,
    a_track: TrackId,
    b_track: TrackId,
    offline: Arc<OfflineSession>,
    queue_a: Arc<Queue>,
    queue_b: Arc<Queue>,
    rendered_frames: i64,
    session: SessionHandle,
    shutdown: CancelScope,
}

struct SelectedAnalysis {
    analysis: TrackAnalysis,
    index: usize,
    track_id: TrackId,
}

struct SyncExpectation {
    primary_bpm: f64,
    secondary_bpm: f64,
    stagger_frames: usize,
}

trait SyncUiControl {
    fn request_sync(&mut self, id: DeckId);
    fn pending_sync(&self) -> Option<DeckId>;
    fn sync_enabled(&self, id: DeckId) -> bool;
}

impl SyncUiControl for Kithara {
    fn request_sync(&mut self, id: DeckId) {
        let letter = match id {
            A => 'a',
            B => 'b',
            _ => panic!("test has only decks A and B"),
        };
        let _task = update::update(
            self,
            Message::Ui(UiEvent::Control {
                path: format!("deck-{letter}/sync"),
                action: ControlAction::Activate,
            }),
        );
    }

    fn pending_sync(&self) -> Option<DeckId> {
        None
    }

    fn sync_enabled(&self, _id: DeckId) -> bool {
        false
    }
}

impl From<(&TrackAnalysis, &TrackAnalysis)> for SyncExpectation {
    fn from((primary, secondary): (&TrackAnalysis, &TrackAnalysis)) -> Self {
        let primary_bpm =
            deck_bpm(primary).expect("production analysis must expose the primary marker tempo");
        let secondary_bpm = deck_bpm(secondary)
            .expect("production analysis must expose the secondary marker tempo");
        let stagger_frames = (f64::from(SAMPLE_RATE) * 60.0 / primary_bpm * STAGGER_BEATS)
            .round()
            .to_usize()
            .expect("analysed stagger fits usize");
        Self {
            primary_bpm,
            secondary_bpm,
            stagger_frames,
        }
    }
}

impl AppTrace {
    async fn new() -> Self {
        let temp = TempDir::new().expect("sync fixture temp directory");
        let a_path = temp.path().join("deck-a-120.wav");
        let b_unused_path = temp.path().join("deck-b-unused.wav");
        let b_path = temp.path().join("deck-b-126.wav");
        write_pulse_wav(&a_path, A_BPM, 330.0, TRACK_SECONDS);
        write_pulse_wav(&b_unused_path, 105.0, 550.0, B_UNUSED_SECONDS);
        write_pulse_wav(&b_path, B_BPM, 880.0, TRACK_SECONDS);

        let shutdown = CancelScope::new(None);
        let app_shutdown = shutdown.token();
        let downloader = Downloader::new(
            DownloaderConfig::for_client(HttpClient::new(
                NetOptions::default(),
                app_shutdown.child(),
            ))
            .cancel(app_shutdown.child())
            .build(),
        );
        let config = AppConfig::builder()
            .store(
                AssetStore::builder()
                    .backend(StorageBackend::Memory)
                    .cancel(app_shutdown.child())
                    .build(),
            )
            .byte_pool(BytePool::default())
            .pcm_pool(PcmPool::default())
            .shutdown(app_shutdown.clone())
            .downloader(downloader)
            .tracks(vec![
                a_path.to_string_lossy().into_owned(),
                b_path.to_string_lossy().into_owned(),
            ])
            .crossfade_seconds(0.0)
            .build();

        let offline = Arc::new(OfflineSession::default());
        let dispatcher = offline.clone() as Arc<dyn SessionDispatcher>;
        let session = SessionHandle::new(dispatcher);
        let mut session_decks = DeckSet::new(vec![
            Deck::build(A, &config, &session),
            Deck::build(B, &config, &session),
        ]);
        session_decks
            .commit(session_decks.mix().clone())
            .expect("initial two-deck mix");
        let queue_a = session_decks
            .deck(A)
            .map(|deck| deck.queue.clone())
            .expect("deck A queue");
        let queue_b = session_decks
            .deck(B)
            .map(|deck| deck.queue.clone())
            .expect("deck B queue");
        for deck in session_decks.decks() {
            deck.player
                .ensure_engine_started()
                .expect("offline player engine");
        }

        let a_track = queue_a.append(a_path.to_string_lossy().into_owned());
        let b_unused = queue_b.append(b_unused_path.to_string_lossy().into_owned());
        let b_track = queue_b.append(b_path.to_string_lossy().into_owned());
        wait_loaded(&queue_a, &[a_track]).await;
        wait_loaded(&queue_b, &[b_unused, b_track]).await;

        let controllers = session_decks
            .decks()
            .iter()
            .map(|deck| {
                let controller = Arc::new(StateController::new(
                    deck.queue.clone(),
                    deck.timestretch.clone(),
                    config.clone(),
                    config.shutdown.child(),
                ));
                (deck.id, controller)
            })
            .collect::<Vec<_>>();
        queue_b
            .select(b_track, Transition::None)
            .expect("deck B must select its second track");

        let palette = config.palette.into();
        let broadcast = Broadcaster::new(session.clone(), app_shutdown.child());
        let app = Kithara {
            broadcast,
            config,
            catalog: Catalog::new(vec![
                a_path.to_string_lossy().into_owned(),
                b_path.to_string_lossy().into_owned(),
            ]),
            session: session_decks,
            decks: Decks::new(controllers).expect("two GUI decks"),
            eq_mode: EqMode::default(),
            palette,
            window_id: window::Id::unique(),
            selected_track: None,
            ui: AppUi::new().expect("compiled app UI"),
        };

        Self {
            _temp: temp,
            app,
            a_track,
            b_track,
            offline,
            queue_a,
            queue_b,
            rendered_frames: 0,
            session,
            shutdown,
        }
    }

    delegate::delegate! {
        to self.app {
            #[call(request_sync)]
            fn press_sync(&mut self, id: DeckId);
            fn pending_sync(&self) -> Option<DeckId>;
            #[call(sync_enabled)]
            fn sync_light(&self, id: DeckId) -> bool;
        }
    }

    async fn wait_for_selected_analysis(
        &mut self,
        id: DeckId,
        track_id: TrackId,
        index: usize,
        seconds: usize,
    ) -> SelectedAnalysis {
        let expected_frames = u64::from(SAMPLE_RATE)
            .checked_mul(u64::try_from(seconds).expect("fixture duration fits u64"))
            .expect("fixture frame extent");
        let started = time::Instant::now();
        while started.elapsed() < Duration::from_secs(120) {
            let selected = self.app.decks.get(id).and_then(|deck| {
                let state = deck.controller.snapshot();
                let selected_id = state
                    .current_track_index
                    .and_then(|selected| state.tracks.get(selected))
                    .map(|track| track.id);
                let analysis = state.analysis.as_ref()?;
                (state.current_track_index == Some(index)
                    && selected_id == Some(track_id)
                    && analysis.source_frames() == expected_frames
                    && deck_bpm(analysis).is_some())
                .then(|| SelectedAnalysis {
                    analysis: analysis.clone(),
                    index,
                    track_id,
                })
            });
            if let Some(selected) = selected {
                self.dispatch(Message::Tick);
                return selected;
            }
            time::sleep(Duration::from_millis(1)).await;
        }

        let snapshot = self
            .app
            .decks
            .get(id)
            .map(|deck| deck.controller.snapshot());
        panic!(
            "deck {id:?} never published production analysis for track {track_id:?} at index {index}: current_index={:?}, current_track={:?}, analysis_frames={:?}, analysis_bpm={:?}",
            snapshot
                .as_ref()
                .and_then(|state| state.current_track_index),
            snapshot.as_ref().and_then(|state| {
                state
                    .current_track_index
                    .and_then(|selected| state.tracks.get(selected))
                    .map(|track| track.id)
            }),
            snapshot
                .as_ref()
                .and_then(|state| state.analysis.as_ref())
                .map(TrackAnalysis::source_frames),
            snapshot
                .as_ref()
                .and_then(|state| state.analysis.as_ref())
                .and_then(deck_bpm),
        );
    }

    async fn warm_and_reset(&mut self) -> bool {
        self.queue_a.play();
        self.queue_b.play();
        let mut heard_pcm = false;
        for _ in 0..WARM_PULL_LIMIT {
            let block = self.render_frames(BLOCK_FRAMES).await;
            heard_pcm |= block.iter().any(|sample| sample.abs() > 0.01);
            if heard_pcm && self.queue_a.is_playing() && self.queue_b.is_playing() {
                break;
            }
        }
        self.queue_a.pause();
        self.queue_b.pause();
        let _ = self.render_frames(BLOCK_FRAMES * 2).await;
        self.queue_a.seek(0.0).expect("reset deck A");
        self.queue_b.seek(0.0).expect("reset deck B");
        for _ in 0..WARM_PULL_LIMIT {
            let _ = self.render_frames(BLOCK_FRAMES).await;
            let reset = [&self.queue_a, &self.queue_b].into_iter().all(|queue| {
                queue
                    .position_seconds()
                    .is_some_and(|position| position <= 0.05)
            });
            if reset {
                self.dispatch(Message::Tick);
                return heard_pcm;
            }
        }
        panic!("fixture decks did not return to their source starts");
    }

    async fn start_staggered(&mut self, stagger_frames: usize) {
        self.queue_a.play();
        let _ = self.render_frames(stagger_frames).await;
        self.queue_b.play();
        self.wait_for_positions().await;
    }

    async fn wait_for_positions(&mut self) {
        for _ in 0..WARM_PULL_LIMIT {
            let _ = self.render_frames(BLOCK_FRAMES).await;
            if self.queue_a.position_seconds().is_some()
                && self.queue_b.position_seconds().is_some()
            {
                return;
            }
        }
        panic!("running decks never exposed source positions");
    }

    fn transport(&self) -> Result<SessionTransportSnapshot, String> {
        match self.session.exec(Cmd::QuerySessionTransport) {
            Ok(Reply::SessionTransport(transport)) => Ok(transport),
            Ok(Reply::Err(error)) => Err(error.to_string()),
            Ok(_) => Err("unexpected session transport reply".to_owned()),
            Err(error) => Err(error.to_string()),
        }
    }

    async fn wait_for_sync_mode(&mut self, id: DeckId) -> bool {
        for _ in 0..SYNC_SETTLE_PULL_LIMIT {
            let _ = self.render_frames(BLOCK_FRAMES).await;
            if self.pending_sync().is_none() && self.sync_light(id) {
                return true;
            }
        }
        false
    }

    async fn capture_deck(&mut self, audible: DeckId) -> PcmCapture {
        self.app
            .session
            .set_muted(A, audible != A)
            .expect("apply deck A capture gain");
        self.app
            .session
            .set_muted(B, audible != B)
            .expect("apply deck B capture gain");
        let _ = self.render_frames(BLOCK_FRAMES).await;
        self.capture().await
    }

    async fn capture_mix(&mut self) -> PcmCapture {
        self.app.session.set_muted(A, false).expect("unmute deck A");
        self.app.session.set_muted(B, false).expect("unmute deck B");
        let _ = self.render_frames(BLOCK_FRAMES).await;
        self.capture().await
    }

    async fn capture(&mut self) -> PcmCapture {
        let start_frame = self.rendered_frames;
        let samples = self
            .render_frames(
                usize::try_from(SAMPLE_RATE).expect("sample rate fits usize") * CAPTURE_SECONDS,
            )
            .await;
        PcmCapture {
            samples,
            start_frame,
        }
    }

    async fn render_frames(&mut self, frames: usize) -> Vec<f32> {
        let mut remaining = frames;
        let mut samples = Vec::with_capacity(frames.saturating_mul(usize::from(CHANNELS)));
        while remaining > 0 {
            let block_frames = remaining.min(BLOCK_FRAMES);
            samples.extend(self.offline.render(block_frames));
            self.rendered_frames += i64::try_from(block_frames).expect("block fits i64");
            self.dispatch(Message::Tick);
            remaining -= block_frames;
            yield_now().await;
        }
        samples
    }

    fn dispatch(&mut self, message: Message) {
        let _task = update::update(&mut self.app, message);
    }
}

impl Drop for AppTrace {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

async fn wait_loaded(queue: &Queue, ids: &[TrackId]) {
    for _ in 0..LOAD_YIELD_LIMIT {
        let mut loaded = true;
        for id in ids {
            match queue.track(*id).map(|track| track.status) {
                Some(TrackStatus::Loaded | TrackStatus::Consumed) => {}
                Some(TrackStatus::Failed(error)) => panic!("track {id:?} failed: {error}"),
                Some(_) | None => loaded = false,
            }
        }
        if loaded {
            return;
        }
        time::sleep(Duration::from_millis(1)).await;
    }
    let states = ids
        .iter()
        .map(|id| queue.track(*id).map(|track| track.status))
        .collect::<Vec<_>>();
    panic!("fixture tracks did not load: {states:?}");
}

fn phase_distance(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    let stagger = (a? - b?).abs();
    let phase = stagger.fract();
    Some(phase.min(1.0 - phase))
}

fn deck_bpm(analysis: &TrackAnalysis) -> Option<f64> {
    let rate = f64::from(analysis.source_sample_rate()?.get());
    let beats = analysis.beat()?.beats();
    let span = beats.last()?.checked_sub(*beats.first()?)?.to_f64()?;
    let intervals = beats.len().checked_sub(1)?.to_f64()?;
    let bpm = 60.0 * rate * intervals / span;
    (bpm.is_finite() && bpm > 0.0).then_some(bpm)
}

fn track_beat_at(analysis: &TrackAnalysis, position: f64) -> Option<f64> {
    let rate = f64::from(analysis.source_sample_rate()?.get());
    let frame = position * rate;
    if !frame.is_finite() || frame < 0.0 {
        return None;
    }
    let beats = analysis.beat()?.beats();
    let first = beats.first()?.to_f64()?;
    let last = beats.last()?.to_f64()?;
    if !(first..=last).contains(&frame) {
        return None;
    }
    if frame == last {
        return beats.len().checked_sub(1)?.to_f64();
    }
    let upper = beats.partition_point(|marker| marker.to_f64().is_some_and(|value| value <= frame));
    let lower = upper.checked_sub(1)?;
    let start = beats.get(lower)?.to_f64()?;
    let end = beats.get(upper)?.to_f64()?;
    let ordinal = lower.to_f64()?;
    Some(ordinal + (frame - start) / (end - start))
}

fn write_pulse_wav(path: &Path, bpm: f64, tone_hz: f64, seconds: usize) {
    let total_frames = usize::try_from(SAMPLE_RATE).expect("sample rate fits usize") * seconds;
    let beat_frames = (f64::from(SAMPLE_RATE) * 60.0 / bpm)
        .round()
        .to_usize()
        .expect("fixture beat period fits usize");
    let burst_frames = (beat_frames / 10).max(1);
    let data_bytes = total_frames
        .checked_mul(usize::from(CHANNELS))
        .and_then(|samples| samples.checked_mul(size_of::<i16>()))
        .expect("fixture WAV size");
    let mut wav = pcm16_header(u32::try_from(data_bytes).expect("fixture WAV data fits u32"));
    wav.reserve(data_bytes);
    for frame in 0..total_frames {
        let into_beat = frame % beat_frames;
        let sample = if into_beat < burst_frames {
            let into_beat = into_beat.to_f64().expect("frame fits f64");
            let burst_frames = burst_frames.to_f64().expect("burst length fits f64");
            let decay = 1.0 - into_beat / burst_frames;
            let phase = TAU * tone_hz * into_beat / f64::from(SAMPLE_RATE);
            (phase.sin() * decay * decay * f64::from(i16::MAX) * 0.25)
                .to_i16()
                .expect("attenuated pulse fits i16")
        } else {
            0
        };
        for _ in 0..CHANNELS {
            wav.extend_from_slice(&sample.to_le_bytes());
        }
    }
    fs::write(path, wav).expect("write deterministic pulse WAV");
}

fn pcm16_header(data_bytes: u32) -> Vec<u8> {
    let bytes_per_sample = 2_u16;
    let byte_rate = SAMPLE_RATE * u32::from(CHANNELS) * u32::from(bytes_per_sample);
    let block_align = CHANNELS * bytes_per_sample;
    let mut bytes = Vec::with_capacity(44);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&CHANNELS.to_le_bytes());
    bytes.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    bytes.extend_from_slice(&byte_rate.to_le_bytes());
    bytes.extend_from_slice(&block_align.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_bytes.to_le_bytes());
    bytes
}

struct PcmCapture {
    samples: Vec<f32>,
    start_frame: i64,
}

struct SyncCaptures {
    unsynced_deck_a: PcmCapture,
    unsynced_deck_b: PcmCapture,
    unsynced_mix: PcmCapture,
    synced_deck_a: PcmCapture,
    synced_deck_b: PcmCapture,
    synced_mix: PcmCapture,
}

struct CochleaPhase {
    bpm: f64,
    beat_frames: Vec<i64>,
}

fn cochlea_report(capture: &PcmCapture, label: &str) -> CochleaPhase {
    assert_cochlea_clean(capture, label);
    let audio = Audio {
        samples: capture.samples.clone(),
        channels: CHANNELS,
        sample_rate: SAMPLE_RATE,
    };
    let tempo = estimate_tempo(&audio, &TempoOpts::default());
    assert_rhythmic(label, &tempo);
    let bpm = tempo.bpm.expect("rhythmic Cochlea report must carry tempo");
    let beat_frames = tempo
        .beats_ms
        .iter()
        .map(|milliseconds| {
            capture.start_frame
                + (milliseconds * f64::from(SAMPLE_RATE) / 1_000.0)
                    .round()
                    .to_i64()
                    .expect("Cochlea beat frame fits i64")
        })
        .collect();
    CochleaPhase { bpm, beat_frames }
}

fn assert_cochlea_clean(capture: &PcmCapture, label: &str) {
    let audio = Audio {
        samples: capture.samples.clone(),
        channels: CHANNELS,
        sample_rate: SAMPLE_RATE,
    };
    let probe_report = probe(&audio, &ProbeOpts::default());
    assert_eq!(
        probe_report.clipping.clipped_samples, 0,
        "{label} Cochlea capture must not clip"
    );
    assert!(
        !probe_report.clipping.true_peak_over_0dbtp,
        "{label} Cochlea capture must stay below 0 dBTP"
    );
}

fn assert_rhythmic(label: &str, report: &TempoReport) {
    assert!(
        report.clear_rhythm && report.bpm.is_some() && report.beats_ms.len() >= 3,
        "{label} Cochlea rhythm is not usable: bpm={:?}, confidence={:.3}, beats={} ",
        report.bpm,
        report.confidence,
        report.beats_ms.len()
    );
}

fn circular_phase(frames: &[i64], period: u64) -> Option<(u64, f64)> {
    if frames.is_empty() || period == 0 {
        return None;
    }
    let period_i64 = i64::try_from(period).ok()?;
    let period_f64 = period.to_f64()?;
    let (sin_sum, cos_sum) = frames.iter().fold((0.0_f64, 0.0_f64), |(sin, cos), frame| {
        let remainder: f64 = frame.rem_euclid(period_i64).as_();
        let angle = remainder / period_f64 * TAU;
        (sin + angle.sin(), cos + angle.cos())
    });
    let concentration = sin_sum.hypot(cos_sum) / frames.len().to_f64()?;
    let angle = sin_sum.atan2(cos_sum).rem_euclid(TAU);
    let phase = (angle / TAU * period_f64).round().to_u64()? % period;
    Some((phase, concentration))
}

fn circular_spread(phases: &[u64], period: u64) -> Option<u64> {
    if phases.len() < 2 || period == 0 {
        return None;
    }
    let mut phases = phases.to_vec();
    phases.sort_unstable();
    let largest_gap = phases
        .windows(2)
        .map(|window| window[1] - window[0])
        .chain(std::iter::once(
            period - phases[phases.len() - 1] + phases[0],
        ))
        .max()?;
    Some(period - largest_gap)
}
