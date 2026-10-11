use super::{cases::*, imports::*, providers::*};

#[derive(Clone, Copy, Debug)]
pub(crate) enum SyncRequest {
    On,
    Off,
    Align,
}

pub(crate) struct ProductHarness {
    pub(crate) decks: Vec<LinkedQueueHandle<TestPools>>,
    pub(crate) failures: Vec<String>,
    block_frames: usize,
    pub(crate) host: OfflineHostHarness<TestPools, LinkedOwner<TestPools>>,
    /// The limited mix before the metronome: what the oracles read.
    master: TapProbe,
    /// Keeps real renderer probes scoped to this harness.
    _trace: usdt_trace::Scope,
    output_frames: u64,
    paced: bool,
    provider: Provider,
    /// The second each deck's start opens it at, before its stagger.
    cues: Vec<f64>,
    /// Every rendered block as `graph_out` plays it, metronome included,
    /// when artifacts are on.
    #[cfg(not(target_os = "android"))]
    tap: Option<kithara_integration_tests::audio_artifact::AudioArtifactTap>,
}

/// Which decks a harness run hears.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Audible {
    /// One deck solo; every other deck is muted.
    Deck(usize),
    /// Every deck.
    Mix,
}

impl Audible {
    const fn hears(self, deck: usize) -> bool {
        match self {
            Self::Deck(solo) => solo == deck,
            Self::Mix => true,
        }
    }

    #[cfg(not(target_os = "android"))]
    fn label(self) -> String {
        match self {
            Self::Deck(deck) => format!("deck-{deck}"),
            Self::Mix => "mix".to_owned(),
        }
    }
}

/// Listening artifacts: an opt-in desktop recording of each harness run with
/// the Host metronome. They encode through the app's recording stack, which no
/// device build carries.
#[cfg(not(target_os = "android"))]
mod artifact {
    use kithara_integration_tests::audio_artifact::{AudioArtifactTap, artifact_label};

    use super::{Audible, CHANNELS, Provider, SyncCase};

    /// The artifact of one harness run, named by the running test, the case and
    /// the decks it hears.
    pub(crate) fn open_tap(
        case: SyncCase,
        provider: Provider,
        audible: Audible,
    ) -> Option<AudioArtifactTap> {
        let mut tap = AudioArtifactTap::from_env(
            &format!("{}-{}-{}", artifact_label(), case.id, audible.label()),
            case.sample_rate,
            CHANNELS,
        )
        .unwrap_or_else(|error| panic!("{}: open the listening artifact: {error}", case.id))?;
        tap.evidence(
            "provider",
            serde_json::Value::String(format!("{provider:?}")),
        );
        Some(tap)
    }
}

impl ProductHarness {
    /// A harness whose decks seek to `start` when the case seeks.
    pub(crate) async fn new(
        case: SyncCase,
        prepared: &PreparedSources,
        start: Start,
        audible: Audible,
    ) -> Self {
        Self::build(case, prepared, start, audible, BLOCK_FRAMES, false).await
    }

    pub(crate) async fn new_for_block(
        case: SyncCase,
        prepared: &PreparedSources,
        start: Start,
        audible: Audible,
        block_frames: usize,
    ) -> Self {
        Self::build(case, prepared, start, audible, block_frames, true).await
    }

    #[kithara_test_utils::kithara::hang_watchdog(timeout = Duration::from_secs(5))]
    async fn build(
        case: SyncCase,
        prepared: &PreparedSources,
        start: Start,
        audible: Audible,
        block_frames: usize,
        paced: bool,
    ) -> Self {
        let provider = prepared.0;
        let sources: Vec<_> = prepared
            .2
            .iter()
            .cycle()
            .take(case.decks)
            .cloned()
            .collect();
        let pools = pools();
        let worker = PlayWorker::new(
            PlayWorkerConfig::builder(pools.clone())
                .maybe_capacity(case.capacity)
                .build(),
        );
        let sample_rate = NonZeroU32::new(case.sample_rate).expect("fixture sample rate");
        let render_block_frames = NonZeroU32::new(
            u32::try_from(block_frames).expect("offline render block count fits u32"),
        )
        .expect("offline render block count is non-zero");
        let session = HostConfig::offline(pools)
            .max_block_frames(render_block_frames)
            .settings(
                HostSettings::builder()
                    .sample_rate(sample_rate)
                    .metronome(
                        MetronomeConfig::builder()
                            .level(METRONOME_DUCK)
                            .duck(METRONOME_DUCK)
                            .build(),
                    )
                    .build(),
            )
            .build();
        let trace = usdt_trace::scope();
        let trajectory = TempoTrajectory::new(
            TempoStep {
                frame: SessionFrame::new(0),
                beat: SessionBeat::default(),
                tempo: Tempo::new(START_BPM).expect("initial tempo"),
            },
            std::num::NonZeroU16::new(4).expect("fixture meter"),
            sample_rate,
        );
        let host_axis = trajectory.clone();
        let host = OfflineHostHarness::layered(session, None, move |core| {
            LinkedHost::new(core, host_axis)
        })
        .await
        .unwrap_or_else(|error| panic!("{}: create offline Host: {error}", case.id));
        host.with(|host| host.metronome().set_enabled(true))
            .await
            .unwrap_or_else(|error| panic!("{}: metronome: {error}", case.id));
        let master = host
            .attach_tap(
                Tap::Master,
                case.sample_rate as usize * usize::from(CHANNELS) * MASTER_TAP_SECONDS,
            )
            .await
            .unwrap_or_else(|error| panic!("{}: master tap: {error}", case.id));
        let mut decks = Vec::with_capacity(sources.len());
        let mut ids = Vec::with_capacity(sources.len());
        for (index, source) in sources.into_iter().enumerate() {
            hang_tick!();
            // Ruling: PlayerImpl-backed Host deck → Queue<LinkedFactory<ObservedFactory>> on LinkedHost — spec §4.4–§4.6, §11.2.
            let observation = Arc::new(Mutex::new(LinkObservation::default()));
            let queue = Queue::new(
                QueueConfig::with_factory(LinkedFactory::new(
                    ObservedFactory::new(observation.clone()),
                    LinkConfig::default(),
                    trajectory.clone(),
                ))
                .should_autoplay(false)
                .prep(
                    ResourcePrep::builder()
                        .worker(worker.clone())
                        .warp(
                            kithara::warp::WarpConfig::builder()
                                .render_quantum_frames(
                                    NonZeroUsize::new(block_frames.min(128))
                                        .expect("fixture render quantum"),
                                )
                                .build(),
                        )
                        .maybe_response_budget_frames(case.response_budget)
                        .build(),
                )
                .store(memory_asset_store())
                .build(),
            );
            let deck = host
                .insert_linked(queue, observation)
                .await
                .unwrap_or_else(|error| panic!("{}: insert deck {index}: {error}", case.id));
            let config = ResourceConfig::for_src(
                ResourceSrc::parse(&source)
                    .unwrap_or_else(|error| panic!("{}: parse source {source}: {error}", case.id)),
            )
            .store(memory_asset_store())
            .initial_abr_mode(AbrMode::manual(0))
            .discriminator(format!("{}-{provider:?}-{index}", case.id))
            .maybe_beat_grid(case.gridded.then(|| provider.beat_grid(index)))
            .build();
            let control = deck.control().clone();
            let muted = !audible.hears(index);
            let id = host
                .run(move || {
                    control.set_muted(muted).expect("deck mute");
                    control.append(TrackSource::Config(Box::new(config)))
                })
                .await
                .unwrap_or_else(|error| panic!("{}: append deck {index}: {error}", case.id));
            decks.push(deck);
            ids.push(id);
            hang_reset!();
        }
        let mut harness = Self {
            decks,
            failures: Vec::new(),
            block_frames,
            host,
            master,
            output_frames: 0,
            paced,
            provider,
            cues: (0..case.decks)
                .map(|deck| provider.start_seconds(deck, start))
                .collect(),
            #[cfg(not(target_os = "android"))]
            tap: artifact::open_tap(case, provider, audible),
            _trace: trace,
        };
        for (index, (deck, id)) in harness.decks.iter().zip(ids.iter().copied()).enumerate() {
            hang_tick!();
            let control = deck.control().clone();
            harness
                .host
                .run(move || {
                    let selected = control.select(id, Transition::None);
                    control.pause();
                    selected
                })
                .await
                .unwrap_or_else(|error| panic!("{}: select deck {index}: {error}", case.id));
            hang_reset!();
        }
        harness.wait_loaded(case, &ids).await;
        harness.deliver_grids(case).await;
        harness.set_tempo(case, case.start_bpm(), true).await;
        harness.warm_up_transport(case).await;
        if !case.paused {
            harness.start_staggered(case).await;
        }
        harness
    }

    /// Render a fixed span so PCM comparisons start on the same frame, then
    /// require a loaded track, its dispatcher receipt and an applied renderer receipt.
    #[kithara_test_utils::kithara::hang_watchdog(timeout = Duration::from_secs(5))]
    async fn warm_up_transport(&mut self, case: SyncCase) {
        let frames = u64::try_from(WARM_UP_BLOCKS * self.block_frames)
            .expect("fixture transport warm-up span");
        while self.output_frames < frames {
            hang_tick!();
            let _ = self.render(case, self.block_frames).await;
            hang_reset!();
        }
        // Ruling: processed TransportRevision probe → real load Settled and published track snapshot — spec §11.2, F belongs to LinkedHost.
        for deck in &self.decks {
            let item = deck.current().expect("selected item").id;
            let snapshot = deck
                .track_snapshot(item)
                .expect("Host publishes its loaded track");
            assert!(
                snapshot.track.attached,
                "{}: renderer attached the loaded track",
                case.id
            );
            deck.observe(|observation| {
                let load = observation
                    .loads
                    .iter()
                    .find_map(|&(owner, seq)| (owner == item).then_some(seq))
                    .expect("real Load sequence");
                assert!(
                    observation.dispatcher.contains(&(
                        load,
                        kithara_integration_tests::offline::linked::LinkVerdict::Applied
                    )),
                    "{}: dispatcher acknowledged that Load",
                    case.id
                );
                assert!(
                    observation
                        .settled
                        .iter()
                        .any(|(owner, receipt)| *owner == item
                            && matches!(receipt, kithara::play::Settled::Applied { .. })),
                    "{}: renderer acknowledged that track",
                    case.id
                );
            });
        }
    }

    #[kithara_test_utils::kithara::hang_watchdog(timeout = Duration::from_secs(5))]
    async fn wait_loaded(&mut self, case: SyncCase, ids: &[kithara::events::TrackId]) {
        let deadline = Instant::now() + LOAD_TIMEOUT;
        let mut completed = 0;
        loop {
            hang_tick!();
            self.tick_all(case).await;
            let mut loaded = true;
            let mut ready = 0;
            for (index, (deck, id)) in self.decks.iter().zip(ids).enumerate() {
                match deck.track(*id).map(|track| track.status) {
                    Some(TrackStatus::Loaded | TrackStatus::Consumed) => ready += 1,
                    Some(TrackStatus::Failed(error)) => {
                        panic!("{}: deck {index} failed to load: {error}", case.id)
                    }
                    Some(_) | None => loaded = false,
                }
                loaded &= deck.current().is_some_and(|track| track.id == *id);
            }
            if loaded {
                hang_reset!();
                return;
            }
            if ready > completed {
                completed = ready;
                hang_reset!();
            }
            assert!(
                Instant::now() < deadline,
                "{}: deck load timed out",
                case.id
            );
            if ready == ids.len() {
                let _ = self.render(case, self.block_frames).await;
            } else {
                time::sleep(Duration::from_millis(1)).await;
            }
        }
    }

    /// Ticks every deck from the host owner thread, as the app update loop
    /// would.
    async fn tick_all(&self, case: SyncCase) {
        let controls: Vec<_> = self
            .decks
            .iter()
            .map(|deck| deck.control().clone())
            .collect();
        self.host
            .run(move || {
                for (index, control) in controls.iter().enumerate() {
                    control
                        .tick()
                        .unwrap_or_else(|error| panic!("{}: tick deck {index}: {error}", case.id));
                }
            })
            .await;
    }

    #[kithara_test_utils::kithara::hang_watchdog(timeout = Duration::from_secs(5))]
    pub(crate) async fn render(&mut self, case: SyncCase, frames: usize) -> Vec<f32> {
        hang_tick!();
        let started = Instant::now();
        self.tick_all(case).await;
        let start = self.output_frames;
        let end = start
            .checked_add(u64::try_from(frames).expect("render frame count fits u64"))
            .expect("offline render timeline fits u64");
        let samples = self.host.render(frames).await;
        assert_eq!(self.host.position(), end);
        hang_reset!();
        self.tick_all(case).await;
        self.output_frames = end;
        #[cfg(not(target_os = "android"))]
        if let Some(tap) = self.tap.as_mut() {
            tap.push(&samples);
        }
        let delay = if self.paced {
            let frames: f64 = frames.as_();
            Duration::from_secs_f64(frames / f64::from(case.sample_rate))
                .saturating_sub(started.elapsed())
        } else {
            Duration::from_millis(1)
        };
        time::sleep(delay).await;
        let master = self.master.drain();
        assert_eq!(self.master.drops(), 0, "the master tap keeps every frame");
        assert_eq!(
            master.len(),
            frames * usize::from(CHANNELS),
            "the master tap sees every rendered frame"
        );
        master
    }

    /// Stamps a control marker on the artifact, when artifacts are on.
    #[cfg(not(target_os = "android"))]
    pub(crate) fn mark(&mut self, label: &str) {
        if let Some(tap) = self.tap.as_mut() {
            tap.mark(label);
        }
    }

    #[kithara_test_utils::kithara::hang_watchdog(timeout = Duration::from_secs(5))]
    pub(crate) async fn settle(&mut self, case: SyncCase, blocks: usize) {
        for _ in 0..blocks {
            hang_tick!();
            let _ = self.render(case, self.block_frames).await;
            hang_reset!();
        }
    }

    pub(crate) async fn play_all(&self) {
        let controls: Vec<_> = self
            .decks
            .iter()
            .map(|deck| deck.control().clone())
            .collect();
        self.host
            .run(move || {
                for control in &controls {
                    control.play();
                }
            })
            .await;
    }

    async fn start_staggered(&mut self, case: SyncCase) {
        let stagger_frames: usize = (f64::from(case.sample_rate) * 3.0 / 8.0 * 60.0
            / case.start_bpm())
        .round()
        .as_();
        for index in 0..self.decks.len() {
            let control = self.decks[index].control().clone();
            self.host.run(move || control.play()).await;
            if index + 1 < self.decks.len() {
                let _ = self.render(case, stagger_frames).await;
            }
        }
        self.settle(case, 2).await;
    }

    delegate::delegate! {
        to self.provider {
            #[call(beat_grid)]
            pub(crate) fn provider_grid(&self, deck: usize) -> ArtifactSource<BeatGridModel>;
            /// The second deck `deck`'s track opens at for `start`.
            pub(crate) fn start_seconds(&self, deck: usize, start: Start) -> f64;
        }
    }

    #[kithara_test_utils::kithara::hang_watchdog(timeout = Duration::from_secs(5))]
    pub(crate) async fn seek_staggered(&mut self, case: SyncCase) {
        let stagger_seconds = 3.0 / 8.0 * 60.0 / case.start_bpm();
        for (index, deck) in self.decks.iter().enumerate() {
            hang_tick!();
            let stagger: f64 = index.as_();
            let seconds = stagger.mul_add(stagger_seconds, self.cues[index]);
            let control = deck.control().clone();
            self.host
                .run(move || control.seek(seconds))
                .await
                .unwrap_or_else(|error| panic!("{}: seek deck {index}: {error}", case.id));
            hang_reset!();
        }
        self.settle(case, 96).await;
    }

    pub(crate) async fn set_tempo(&mut self, case: SyncCase, bpm: f64, required: bool) {
        let tempo = Tempo::new(bpm).expect("fixture tempo");
        match self.host.with(move |host| host.set_tempo(tempo)).await {
            Ok(()) => {}
            Err(error) if !required => self.record_tempo_failure(format!(
                "tempo request {bpm:.6} BPM could not reach Host: {error}"
            )),
            Err(error) => panic!("{}: set initial tempo: {error}", case.id),
        }
    }

    fn record_tempo_failure(&mut self, failure: String) {
        if !self
            .failures
            .iter()
            .any(|failure| failure.starts_with("tempo request"))
        {
            self.failures.push(failure);
        }
    }

    async fn deliver_grids(&self, case: SyncCase) {
        // Ruling: SyncOperation source/load grid topology → item/load-scoped GridAnswer — spec §4.8, §11.2.
        for (index, deck) in self.decks.iter().enumerate() {
            let item = deck.current().expect("selected item").id;
            let load = deck
                .observe(|observation| {
                    observation
                        .loads
                        .iter()
                        .rev()
                        .find(|(loaded, _)| *loaded == item)
                        .map(|(_, seq)| *seq)
                })
                .expect("real load sequence");
            let ArtifactSource::Value(model) = self.provider.beat_grid(index) else {
                panic!("fixture grid must be materialized")
            };
            let target = deck.id();
            self.host
                .with(move |host| {
                    host.send(LinkedHostCommand::Grid {
                        deck: target,
                        answer: GridAnswer {
                            item,
                            load,
                            model: Ok((*model).clone()),
                        },
                    })
                })
                .await
                .unwrap_or_else(|error| panic!("{}: deliver grid {index}: {error}", case.id));
        }
    }

    pub(crate) async fn request_sync(&mut self, case: SyncCase) {
        self.request_sync_intent(case, SyncRequest::On).await;
    }

    pub(crate) async fn request_sync_intent(&mut self, case: SyncCase, intent: SyncRequest) {
        // Ruling: SyncIntent::Enable → LinkedHostCommand::Sync{on:true} — spec §4.5–§4.6.
        // Ruling: SyncIntent::Disable → LinkedHostCommand::Sync{on:false} — spec §4.5–§4.6.
        // Ruling: SyncIntent::AlignNow → explicit Sync{on:true}, including a repeated enable — spec §4.5.
        // Ruling: SyncOperation::Sync{source,activation,transport,load} → DeckId-routed Sync; Linked chooses F from its observed mark and lead — spec §3.4, §4.5, §11.2.
        #[cfg(not(target_os = "android"))]
        self.mark(&format!("sync {intent:?}"));
        for index in 0..self.decks.len() {
            let target = self.decks[index].id();
            let on = !matches!(intent, SyncRequest::Off);
            self.host
                .with(move |host| host.send(LinkedHostCommand::Sync { deck: target, on }))
                .await
                .unwrap_or_else(|error| panic!("{}: sync deck {index}: {error}", case.id));
            if matches!(case.order, OperationOrder::SequentialSync) {
                let _ = self.render(case, self.block_frames).await;
            }
        }
    }

    pub(crate) async fn run_operations(&mut self, case: SyncCase) {
        for operation in case.order.operations() {
            match operation {
                Operation::Play => {
                    self.play_all().await;
                    self.settle(case, 2).await;
                }
                Operation::Seek => self.seek_staggered(case).await,
                Operation::Sync => self.request_sync(case).await,
            }
        }
    }

    #[kithara_test_utils::kithara::hang_watchdog(timeout = Duration::from_secs(5))]
    pub(crate) async fn ride_tempo(&mut self, case: SyncCase) {
        let steps_per_leg = (case.updates_hz / 2).max(1);
        let mut start = case.start_bpm();
        let mut rendered = 0_u64;
        let mut update = 0_u64;
        for &target in case.ride.points() {
            for step in 1..=steps_per_leg {
                hang_tick!();
                let fraction = f64::from(step) / f64::from(steps_per_leg);
                let bpm = start + (target - start) * fraction;
                self.set_tempo(case, bpm, false).await;
                update += 1;
                let deadline = update * u64::from(case.sample_rate) / u64::from(case.updates_hz);
                let frames = deadline.saturating_sub(rendered);
                rendered = deadline;
                if frames > 0 {
                    let frames = usize::try_from(frames).expect("tempo interval fits usize");
                    let _ = self.render(case, frames).await;
                }
                hang_reset!();
            }
            start = target;
        }
        self.settle(case, 4).await;
    }

    pub(super) async fn capture(&mut self, case: SyncCase) -> Vec<f32> {
        self.play_all().await;
        self.settle(case, 4).await;
        let capture_frames: usize = (f64::from(case.sample_rate) * 60.0 / case.ride.final_bpm()
            * 6.0)
            .round()
            .as_();
        self.capture_frames(case, capture_frames, self.block_frames)
            .await
    }

    pub(crate) async fn capture_frames(
        &mut self,
        case: SyncCase,
        capture_frames: usize,
        block_frames: usize,
    ) -> Vec<f32> {
        self.capture_blocks(case, capture_frames, block_frames, false)
            .await
    }

    pub(crate) async fn capture_paced(
        &mut self,
        case: SyncCase,
        capture_frames: usize,
    ) -> Vec<f32> {
        self.capture_blocks(case, capture_frames, self.block_frames, true)
            .await
    }

    #[kithara_test_utils::kithara::hang_watchdog(timeout = Duration::from_secs(5))]
    async fn capture_blocks(
        &mut self,
        case: SyncCase,
        capture_frames: usize,
        block_frames: usize,
        paced: bool,
    ) -> Vec<f32> {
        let capture_kind = if paced { "paced" } else { "product" };
        let mut pcm = Vec::with_capacity(capture_frames * usize::from(CHANNELS));
        while pcm.len() < capture_frames * usize::from(CHANNELS) {
            hang_tick!();
            let remaining_frames =
                (capture_frames * usize::from(CHANNELS) - pcm.len()) / usize::from(CHANNELS);
            let frames = remaining_frames.min(block_frames);
            let started = paced.then(Instant::now);
            let block = self.render(case, frames).await;
            assert!(
                !block.is_empty(),
                "{}: {capture_kind} capture stopped making PCM progress",
                case.id,
            );
            pcm.extend(block);
            hang_reset!();
            if let Some(started) = started {
                let frames: f64 = frames.as_();
                let period = Duration::from_secs_f64(frames / f64::from(case.sample_rate));
                time::sleep(period.saturating_sub(started.elapsed())).await;
            }
        }
        pcm
    }
}
