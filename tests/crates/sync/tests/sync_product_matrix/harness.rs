use kithara::{
    audio::{
        AudioDecoderConfig, DecoderBackend as EventDecoderBackend, DecoderChangeCause, DecoderEvent,
    },
    decode::DecoderBackend,
    events::EventBus,
};

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
    worker: PlayWorker<TestPools>,
    trajectory: TempoTrajectory,
    operation_grid: Option<kithara::warp::BeatGridSnapshot>,
    capture_marks: Vec<(
        SessionFrame,
        kithara::events::TrackId,
        kithara_render::bridge::SlotSnapshot,
    )>,
    host_grids: Vec<(SessionFrame, kithara::warp::BeatGridSnapshot)>,
    /// The second each deck's start opens it at, before its stagger.
    cues: Vec<f64>,
    decoder_backends: Vec<EventDecoderBackend>,
    decoder_rates: Vec<Option<(u32, u32)>>,
    /// Every rendered block as `graph_out` plays it, metronome included.
    #[cfg(not(target_os = "android"))]
    tap: kithara_integration_tests::audio_artifact::AudioArtifactTap,
    #[cfg(not(target_os = "android"))]
    master_tap: kithara_integration_tests::audio_artifact::AudioArtifactTap,
    #[cfg(not(target_os = "android"))]
    metronome_delta_peak: f32,
}

fn linked_queue(
    case: SyncCase,
    worker: &PlayWorker<TestPools>,
    trajectory: &TempoTrajectory,
    observation: &Arc<Mutex<LinkObservation>>,
    block_frames: usize,
) -> kithara_integration_tests::offline::linked::LinkedQueue<TestPools> {
    Queue::new(
        QueueConfig::with_factory(LinkedFactory::new(
            LinkedConfig::builder()
                .inner(ObservedFactory::new(observation.clone()))
                .settings(LinkConfig::default())
                .trajectory(trajectory.clone())
                .build(),
        ))
        .should_autoplay(false)
        .prep(
            ResourcePrep::builder()
                .worker(worker.clone())
                .block_on_underrun(true)
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
    )
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

/// Listening artifacts of each desktop harness run with the Host metronome.
/// They encode through the app's recording stack, which no device build carries.
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
        stream: &str,
    ) -> AudioArtifactTap {
        let default_root = std::env::current_exe()
            .unwrap_or_else(|error| panic!("{}: locate the test executable: {error}", case.id))
            .with_file_name("audio-artifacts");
        let mut tap = AudioArtifactTap::from_env_or(
            &default_root,
            &format!(
                "{}-{}-{}-{stream}",
                artifact_label(),
                case.id,
                audible.label()
            ),
            case.sample_rate,
            CHANNELS,
        )
        .unwrap_or_else(|error| panic!("{}: open the listening artifact: {error}", case.id));
        tap.evidence(
            "provider",
            serde_json::Value::String(format!("{provider:?}")),
        );
        tap
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
        Self::build(case, prepared, start, audible, BLOCK_FRAMES, false, None).await
    }

    pub(crate) async fn new_for_block(
        case: SyncCase,
        prepared: &PreparedSources,
        start: Start,
        audible: Audible,
        block_frames: usize,
    ) -> Self {
        Self::build(case, prepared, start, audible, block_frames, true, None).await
    }

    pub(crate) async fn new_with_decoder(
        case: SyncCase,
        prepared: &PreparedSources,
        start: Start,
        audible: Audible,
        decoder: DecoderBackend,
    ) -> Self {
        Self::build(
            case,
            prepared,
            start,
            audible,
            BLOCK_FRAMES,
            false,
            Some(decoder),
        )
        .await
    }

    pub(crate) fn assert_decoder_backend(&self, expected: EventDecoderBackend) {
        assert_eq!(self.decoder_backends.len(), self.decks.len());
        assert!(
            self.decoder_backends
                .iter()
                .all(|backend| *backend == expected),
            "requested decoder must own every source: {:?}",
            self.decoder_backends
        );
    }

    #[kithara_test_utils::kithara::hang_watchdog(timeout = Duration::from_secs(5), ctx = String)]
    async fn build(
        case: SyncCase,
        prepared: &PreparedSources,
        start: Start,
        audible: Audible,
        block_frames: usize,
        paced: bool,
        decoder: Option<DecoderBackend>,
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
            LinkedHost::new(
                LinkedHostConfig::builder()
                    .inner(core)
                    .trajectory(host_axis)
                    .build(),
            )
        })
        .await
        .unwrap_or_else(|error| panic!("{}: create offline Host: {error}", case.id));
        host.with(|host| host.metronome().set_enabled(true))
            .await
            .unwrap_or_else(|error| panic!("{}: metronome: {error}", case.id));
        hang_tick!(format!(
            "{}: publishing the offline clock and stream shape",
            case.id
        ));
        let _ = host.render(block_frames).await;
        hang_reset!();
        let output_frames = host.position();
        let master = host
            .attach_tap(
                Tap::Master,
                case.sample_rate as usize * usize::from(CHANNELS) * MASTER_TAP_SECONDS,
            )
            .await
            .unwrap_or_else(|error| panic!("{}: master tap: {error}", case.id));
        let mut decks = Vec::with_capacity(sources.len());
        let mut ids = Vec::with_capacity(sources.len());
        let mut decoder_receivers = Vec::new();
        for (index, source) in sources.into_iter().enumerate() {
            hang_tick!(format!("{}: creating deck {index}", case.id));
            // Ruling: PlayerImpl-backed Host deck → Queue<LinkedFactory<ObservedFactory>> on LinkedHost — spec §4.4–§4.6, §11.2.
            let observation = Arc::new(Mutex::new(LinkObservation::default()));
            let queue = linked_queue(case, &worker, &trajectory, &observation, block_frames);
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
            .maybe_beat_grid(case.gridded.then(|| provider.beat_grid(index)));
            let events = EventBus::default();
            decoder_receivers.push(events.subscribe::<DecoderEvent>());
            let config = config.events(events);
            let config = if let Some(backend) = decoder {
                config
                    .decoder(AudioDecoderConfig::builder().backend(backend).build())
                    .build()
            } else {
                config.build()
            };
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
            output_frames,
            paced,
            provider,
            worker,
            trajectory,
            operation_grid: None,
            capture_marks: Vec::new(),
            host_grids: Vec::new(),
            decoder_backends: Vec::new(),
            decoder_rates: Vec::new(),
            cues: (0..case.decks)
                .map(|deck| provider.start_seconds(deck, start))
                .collect(),
            #[cfg(not(target_os = "android"))]
            tap: artifact::open_tap(case, provider, audible, "output"),
            #[cfg(not(target_os = "android"))]
            master_tap: artifact::open_tap(case, provider, audible, "master"),
            #[cfg(not(target_os = "android"))]
            metronome_delta_peak: 0.0,
            _trace: trace,
        };
        let _ = harness.render(case, block_frames).await;
        hang_reset!();
        harness.wait_loaded(case, &ids).await;
        for (deck, mut receiver) in decoder_receivers.into_iter().enumerate() {
            let event = receiver
                .try_recv()
                .expect("the loaded source publishes its initial decoder")
                .event;
            if decoder.is_some() {
                eprintln!("DECODER_RECEIPT deck={deck} requested={decoder:?} event={event:?}");
            }
            let DecoderEvent::DecoderChanged {
                backend,
                cause: DecoderChangeCause::Initial,
                ..
            } = event
            else {
                panic!(
                    "the first source decoder event must identify the initial generation: {event:?}"
                );
            };
            harness.decoder_backends.push(backend);
            let rates =
                std::iter::from_fn(|| receiver.try_recv().ok()).find_map(|event| {
                    match event.event {
                        DecoderEvent::ResamplerConfigured {
                            input_rate,
                            output_rate,
                            ..
                        } => Some((input_rate, output_rate)),
                        _ => None,
                    }
                });
            harness.decoder_rates.push(rates);
        }
        harness.deliver_grids(case).await;
        harness.set_tempo(case, case.start_bpm(), true).await;
        harness.warm_up_transport(case).await;
        if !case.paused {
            harness.start_staggered(case).await;
        }
        harness
    }

    #[kithara_test_utils::kithara::hang_watchdog(timeout = Duration::from_secs(5), ctx = String)]
    pub(crate) async fn add_seated_deck(&mut self, case: SyncCase) -> usize {
        let index = self.decks.len();
        hang_tick!(format!(
            "{}: inserting capacity contender deck {index}",
            case.id
        ));
        let observation = Arc::new(Mutex::new(LinkObservation::default()));
        let queue = linked_queue(
            case,
            &self.worker,
            &self.trajectory,
            &observation,
            self.block_frames,
        );
        let deck = self
            .host
            .insert_linked(queue, observation)
            .await
            .unwrap_or_else(|error| panic!("{}: insert capacity contender: {error}", case.id));
        hang_reset!();
        self.decks.push(deck);
        index
    }

    /// Render a fixed span so PCM comparisons start on the same frame, then
    /// require a loaded track, its dispatcher receipt and an applied renderer receipt.
    #[kithara_test_utils::kithara::hang_watchdog(timeout = Duration::from_secs(5), ctx = String)]
    async fn warm_up_transport(&mut self, case: SyncCase) {
        let frames = u64::try_from(WARM_UP_BLOCKS * self.block_frames)
            .expect("fixture transport warm-up span");
        while self.output_frames < frames {
            hang_tick!(format!(
                "{}: warming frame {} to {frames}",
                case.id, self.output_frames
            ));
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

    #[kithara_test_utils::kithara::hang_watchdog(timeout = Duration::from_secs(5), ctx = String)]
    async fn wait_loaded(&mut self, case: SyncCase, ids: &[kithara::events::TrackId]) {
        let deadline = Instant::now() + LOAD_TIMEOUT;
        let mut completed = 0;
        loop {
            hang_tick!(format!(
                "{}: awaiting loads {ids:?}, {completed} ready",
                case.id
            ));
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
                let _real_io = kithara::platform::flash::real_io();
                time::sleep(Duration::from_millis(1).min(__hang_detector.remaining())).await;
            }
        }
    }

    /// Ticks every deck from the host owner thread, as the app update loop
    /// would.
    async fn tick_all(&self, case: SyncCase) -> kithara::warp::BeatGridSnapshot {
        use kithara::warp::BeatGrid;
        let controls: Vec<_> = self
            .decks
            .iter()
            .map(|deck| deck.control().clone())
            .collect();
        self.host
            .with(move |host| {
                for (index, control) in controls.iter().enumerate() {
                    control
                        .tick()
                        .unwrap_or_else(|error| panic!("{}: tick deck {index}: {error}", case.id));
                }
                host.snapshot()
            })
            .await
    }

    #[kithara_test_utils::kithara::hang_watchdog(timeout = Duration::from_secs(5), ctx = String)]
    pub(crate) async fn render(&mut self, case: SyncCase, frames: usize) -> Vec<f32> {
        hang_tick!(format!(
            "{}: rendering {frames} frames from {}",
            case.id, self.output_frames
        ));
        let started = Instant::now();
        let grid = self.tick_all(case).await;
        let start_frame = SessionFrame::new(
            i64::try_from(self.output_frames).expect("observed Host frame fits its session axis"),
        );
        self.host_grids.push((start_frame, grid));
        let start = self.output_frames;
        let end = start
            .checked_add(u64::try_from(frames).expect("render frame count fits u64"))
            .expect("offline render timeline fits u64");
        let samples = self.host.render(frames).await;
        assert_eq!(self.host.position(), end);
        hang_reset!();
        let grid = self.tick_all(case).await;
        let end_frame = SessionFrame::new(
            i64::try_from(end).expect("observed Host frame fits its session axis"),
        );
        self.host_grids.push((end_frame, grid));
        self.output_frames = end;
        #[cfg(not(target_os = "android"))]
        self.tap.push(&samples);
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
        #[cfg(not(target_os = "android"))]
        {
            self.master_tap.push(&master);
            self.metronome_delta_peak = samples
                .iter()
                .zip(&master)
                .map(|(output, master)| (output - master).abs())
                .fold(self.metronome_delta_peak, f32::max);
            self.tap.evidence(
                "metronome_output_delta_peak",
                serde_json::json!(self.metronome_delta_peak),
            );
        }
        master
    }

    /// Stamps a control marker on the listening artifact.
    #[cfg(not(target_os = "android"))]
    pub(crate) fn mark(&mut self, label: &str) {
        self.tap.mark(label);
        self.master_tap.mark(label);
    }

    #[kithara_test_utils::kithara::hang_watchdog(timeout = Duration::from_secs(5), ctx = String)]
    pub(crate) async fn settle(&mut self, case: SyncCase, blocks: usize) {
        for _ in 0..blocks {
            hang_tick!(format!(
                "{}: settling at frame {}",
                case.id, self.output_frames
            ));
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

    #[kithara_test_utils::kithara::hang_watchdog(timeout = Duration::from_secs(5), ctx = String)]
    async fn start_staggered(&mut self, case: SyncCase) {
        let stagger_frames: usize = (f64::from(case.sample_rate) * 3.0 / 8.0 * 60.0
            / case.start_bpm())
        .round()
        .as_();
        for index in 0..self.decks.len() {
            hang_tick!(format!("start_staggered: deck {index}"));
            let control = self.decks[index].control().clone();
            self.host.run(move || control.play()).await;
            if index + 1 < self.decks.len() {
                let _ = self.render(case, stagger_frames).await;
            }
            hang_reset!();
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

    #[kithara_test_utils::kithara::hang_watchdog(timeout = Duration::from_secs(5), ctx = String)]
    pub(crate) async fn seek_staggered(&mut self, case: SyncCase) {
        let stagger_seconds = 3.0 / 8.0 * 60.0 / case.start_bpm();
        for (index, deck) in self.decks.iter().enumerate() {
            hang_tick!(format!("{}: seeking deck {index}", case.id));
            let stagger: f64 = index.as_();
            let seconds = stagger.mul_add(stagger_seconds, self.cues[index]);
            #[cfg(not(target_os = "android"))]
            {
                self.tap.mark(&format!("seek deck {index} to {seconds:.9}"));
                self.master_tap
                    .mark(&format!("seek deck {index} to {seconds:.9}"));
            }
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
        #[cfg(not(target_os = "android"))]
        self.mark(&format!("tempo {bpm:.9}"));
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

    #[kithara_test_utils::kithara::hang_watchdog(timeout = Duration::from_secs(5), ctx = String)]
    async fn deliver_grids(&self, case: SyncCase) {
        // Ruling: SyncOperation source/load grid topology → item/load-scoped GridAnswer — spec §4.8, §11.2.
        for (index, deck) in self.decks.iter().enumerate() {
            hang_tick!(format!("deliver_grids: deck {index}"));
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
            hang_reset!();
        }
    }

    pub(crate) async fn grid_binding_failures(&mut self, case: SyncCase) -> Vec<String> {
        use kithara::play::TrackStatus as OwnerTrackStatus;
        let host_grid = self.host.session_grid().await;
        let mut failures = Vec::new();
        let mut evidence = Vec::new();
        for (index, deck) in self.decks.iter().enumerate() {
            let item = deck.current().expect("selected binding track").id;
            let snapshot = deck.track_snapshot(item).expect("published binding track");
            assert!(matches!(
                snapshot.track.status,
                OwnerTrackStatus::Playing { .. }
            ));
            assert!(
                matches!(
                    snapshot.sync,
                    kithara::link::SyncStatus::On | kithara::link::SyncStatus::Correcting { .. }
                ),
                "a playing synchronized track must expose its steady binding or finite correction"
            );
            let (slot, evaluation_rate) = deck
                .observe(|observation| {
                    let slot = observation
                        .consumed
                        .iter()
                        .find_map(|(owner, slot)| (*owner == item).then_some(*slot))?;
                    Some((slot, observation.consumed_rate?))
                })
                .expect("real DeckPass consumed slot and grid evaluation axis");
            assert_eq!(
                snapshot.track.mark, slot.mark,
                "the Player snapshot reads that consumed PCM mark"
            );
            assert_eq!(
                evaluation_rate.get(),
                case.sample_rate,
                "the consumed DeckPass declares the grid evaluation axis"
            );
            let ArtifactSource::Value(model) = self.provider.beat_grid(index) else {
                panic!("materialized supplied source grid");
            };
            let (capture_frame, initial) = self
                .capture_marks
                .iter()
                .find_map(|(frame, owner, slot)| (*owner == item).then_some((*frame, *slot)))
                .expect("unchanged capture's actual initial consumed mark");
            let Some(capture_start) = self.capture_marks.iter().find_map(|(_, owner, slot)| {
                (*owner == item && slot.state == kithara_render::bridge::SlotState::Playing)
                    .then_some(*slot)
            }) else {
                failures.push(format!(
                    "deck {index}: original capture has no actual consumed Playing slot"
                ));
                continue;
            };
            if initial.state != kithara_render::bridge::SlotState::Playing {
                let first = capture_start
                    .mark
                    .expect("actual Playing capture has a consumed mark");
                let started = deck.observe(|observation| observation.starts.iter().any(|(owner, seq)| *owner == item
                    && observation.placement_receipts.iter().any(|(owner, origin, receipt)| *owner == item
                        && *origin == kithara_integration_tests::offline::linked::LinkReceiptOrigin::Deck
                        && matches!(receipt, kithara::play::Settled::Applied { seq: answered, at }
                            if answered == seq && *at >= capture_frame && *at <= first.session))));
                if !started
                    || initial.media_origin != capture_start.media_origin
                    || initial
                        .mark
                        .is_some_and(|mark| mark.lane.segment != first.lane.segment)
                {
                    failures.push(format!("deck {index}: initially stopped capture lacks real owned Start Applied custody or stable source origin/Segment"));
                    continue;
                }
            }
            let law = deck.observe(|observation| {
                super::binding::executed_law(
                    &model,
                    &host_grid,
                    &self.host_grids,
                    observation,
                    &snapshot,
                    capture_start,
                    slot,
                )
            });
            let actual_speed = match law {
                Ok((speed, law_failures, law_evidence)) => {
                    failures.extend(
                        law_failures
                            .into_iter()
                            .map(|failure| format!("deck {index}: {failure}")),
                    );
                    evidence.push(law_evidence);
                    speed
                }
                Err(error) => {
                    failures.push(format!("deck {index}: {error}"));
                    continue;
                }
            };
            if matches!(snapshot.sync, kithara::link::SyncStatus::Correcting { .. }) {
                continue;
            }
            let binding = super::binding::check(
                &model,
                evaluation_rate,
                &host_grid,
                slot,
                actual_speed.as_(),
                4,
            );
            match binding {
                Ok(binding) => {
                    failures.extend(
                        binding
                            .failures()
                            .into_iter()
                            .map(|failure| format!("deck {index}: {failure}")),
                    );
                    evidence.push(binding.evidence());
                }
                Err(error) => failures.push(format!("deck {index}: {error}")),
            }
        }
        #[cfg(not(target_os = "android"))]
        self.master_tap
            .evidence("grid_bindings", serde_json::json!(evidence));
        failures
    }

    pub(crate) async fn grid_anchor_failures(
        &mut self,
        case: SyncCase,
        required_placements: Option<usize>,
    ) -> Vec<String> {
        use kithara::play::Settled;
        use kithara_integration_tests::offline::linked::LinkReceiptOrigin;

        let host_grid = self.host.session_grid().await;
        let Some(selection_grid) = self.operation_grid.as_ref() else {
            return vec!["no published Host grid was captured before the operations".to_owned()];
        };
        let mut failures = Vec::new();
        let mut evidence = Vec::new();
        let stagger_seconds = 3.0 / 8.0 * 60.0 / case.start_bpm();
        for (index, deck) in self.decks.iter().enumerate() {
            let item = deck.current().expect("selected anchor track").id;
            let ArtifactSource::Value(model) = self.provider.beat_grid(index) else {
                panic!("declared anchor source grid");
            };
            let stagger: f64 = index.as_();
            let requested = Duration::from_secs_f64(self.cues[index] + stagger * stagger_seconds);
            let result = deck.observe(|observation| {
                let evaluation_rate = observation.consumed_rate
                    .ok_or_else(|| "no consumed grid evaluation axis".to_owned())?;
                let consumed = observation.consumed.iter()
                    .find_map(|(owner, slot)| (*owner == item).then_some(*slot))
                    .ok_or_else(|| "no actual consumed anchor slot".to_owned())?;
                let placements = observation.placements.iter()
                    .filter(|placement| placement.item == item).collect::<Vec<_>>();
                let required_placements = super::binding::relocation_count(
                    observation, item, &model, case, required_placements,
                )?;
                if required_placements.is_some_and(|required| placements.len() != required) {
                    return Err(format!(
                        "expected {:?} admitted relocations, found {}",
                        required_placements, placements.len()
                    ));
                }
                let mut bindings = Vec::new();
                for (ordinal, placement) in placements.iter().enumerate() {
                    let required = super::binding::relocation_request(
                        case.order, ordinal + 1 == placements.len(), requested,
                        placement.planned.as_ref()
                            .map(|projected| projected.0)
                            .map_err(|error| format!("pre-emission source projection failed: {error}")),
                    )?;
                    let lead = placement.delivery.get()
                        .checked_add(placement.before.ring_depth.get())
                        .and_then(|lead| lead.checked_add(placement.before.engine_latency.get()))
                        .and_then(|lead| lead.checked_add(placement.before.declick.get()))
                        .ok_or_else(|| "placement lead does not fit the Host frame axis".to_owned())?;
                    let expected_at = placement.now + kithara::signal::FrameCount::new(lead);
                    let anchor = super::binding::select_nearest(
                        &model, evaluation_rate, selection_grid, required, expected_at, 4,
                    )?;
                    let errors = anchor.command_failures(
                        &model, evaluation_rate, selection_grid, placement.to, placement.at,
                    )?;
                    if !errors.is_empty() {
                        return Err(format!("placement {:?}: {}", placement.seq, errors.join("; ")));
                    }
                    let admitted_at = anchor.frame - placement.before.declick;
                    let applied = observation.placement_receipts.iter()
                        .filter(|(owner, origin, settled)| {
                            *owner == item && *origin == LinkReceiptOrigin::Lane
                                && matches!(settled, Settled::Applied { seq, at }
                                    if *seq == placement.seq && *at == admitted_at)
                        }).count();
                    if placement.aligned && applied != 1 {
                        return Err(format!("relocation {:?} has {applied} owned Lane Applied receipts at {:?}", placement.seq, admitted_at));
                    }
                    if !placement.aligned {
                        let applied = observation.placement_applied.iter()
                            .find_map(|(owner, seq, applied)| (*owner == item && *seq == placement.seq).then_some(*applied));
                        if applied != Some(true) {
                            return Err(format!("Jump {:?} has no owned Applied outcome: {:?}", placement.seq, applied));
                        }
                    }
                    let segment = placement.consumed.mark
                        .ok_or_else(|| "admitted relocation has no prior consumed segment".to_owned())?.lane.segment;
                    if ordinal + 1 == placements.len() {
                        let status = observation.tracks.iter().find(|track| track.track.item == item)
                            .ok_or_else(|| "selected relocation has no published Linked snapshot".to_owned())?.sync;
                        let errors = if matches!(status, kithara::link::SyncStatus::Correcting { .. }) {
                            anchor.corrected_consumption_failures(&model, &host_grid, consumed, segment, observation, item)?
                        } else {
                            anchor.consumption_failures(&model, evaluation_rate, &host_grid, consumed, segment)?
                        };
                        if !errors.is_empty() {
                            return Err(errors.join("; "));
                        }
                    }
                    bindings.push(serde_json::json!({
                        "item": u64::from(item), "seq": placement.seq.get(),
                        "requested_media_ns": required.as_nanos().to_string(),
                        "selected_media_ns": anchor.media.as_nanos().to_string(),
                        "source_beat": anchor.source_beat, "host_beat": anchor.host_beat,
                        "target_frame": i64::from(anchor.frame),
                        "scheduled_lane_frame": i64::from(admitted_at), "segment": segment.get(),
                        "lane_applied_frame": placement.aligned.then_some(i64::from(admitted_at)),
                        "aligned": placement.aligned,
                        "owned_applied": observation.placement_applied.iter().find_map(|(owner, seq, applied)|
                            (*owner == item && *seq == placement.seq).then_some(*applied)),
                        "evaluation_rate": evaluation_rate.get(),
                        "selection_grid": format!("{:?}", selection_grid.stamp()),
                        "consumption_grid": format!("{:?}", host_grid.stamp()),
                        "decoder_rates": self.decoder_rates[index],
                        "consumed_mark": format!("{:?}", consumed.mark),
                        "media_origin_ns": consumed.media_origin.map(|origin| origin.as_nanos().to_string()),
                    }));
                }
                Ok(bindings)
            });
            match result {
                Ok(binding) => evidence.push(binding),
                Err(error) => failures.push(format!("deck {index}: {error}")),
            }
        }
        #[cfg(not(target_os = "android"))]
        self.master_tap
            .evidence("grid_anchors", serde_json::json!(evidence));
        failures
    }

    pub(crate) async fn request_sync(&mut self, case: SyncCase) {
        self.request_sync_intent(case, SyncRequest::On).await;
    }

    #[kithara_test_utils::kithara::hang_watchdog(timeout = Duration::from_secs(5), ctx = String)]
    pub(crate) async fn request_sync_intent(&mut self, case: SyncCase, intent: SyncRequest) {
        // Ruling: SyncIntent::Enable → LinkedHostCommand::Sync{on:true} — spec §4.5–§4.6.
        // Ruling: SyncIntent::Disable → LinkedHostCommand::Sync{on:false} — spec §4.5–§4.6.
        // Ruling: SyncIntent::AlignNow → explicit Sync{on:true}, including a repeated enable — spec §4.5.
        // Ruling: SyncOperation::Sync{source,activation,transport,load} → DeckId-routed Sync; Linked chooses F from its observed mark and lead — spec §3.4, §4.5, §11.2.
        #[cfg(not(target_os = "android"))]
        self.mark(&format!("sync {intent:?}"));
        for index in 0..self.decks.len() {
            hang_tick!(format!("request_sync_intent: deck {index}, {intent:?}"));
            let target = self.decks[index].id();
            let on = !matches!(intent, SyncRequest::Off);
            self.host
                .with(move |host| host.send(LinkedHostCommand::Sync { deck: target, on }))
                .await
                .unwrap_or_else(|error| panic!("{}: sync deck {index}: {error}", case.id));
            if matches!(case.order, OperationOrder::SequentialSync) {
                let _ = self.render(case, self.block_frames).await;
            }
            hang_reset!();
        }
    }

    #[kithara_test_utils::kithara::hang_watchdog(timeout = Duration::from_secs(5), ctx = String)]
    pub(crate) async fn run_operations(&mut self, case: SyncCase) {
        self.operation_grid = Some(self.host.session_grid().await);
        for operation in case.order.operations() {
            hang_tick!(format!("run_operations: {operation:?}"));
            match operation {
                Operation::Play => {
                    self.play_all().await;
                    self.settle(case, 2).await;
                }
                Operation::Seek => self.seek_staggered(case).await,
                Operation::Sync => self.request_sync(case).await,
            }
            hang_reset!();
        }
    }

    #[kithara_test_utils::kithara::hang_watchdog(timeout = Duration::from_secs(5), ctx = String)]
    pub(crate) async fn ride_tempo(&mut self, case: SyncCase) {
        let steps_per_leg = (case.updates_hz / 2).max(1);
        let mut start = case.start_bpm();
        let mut rendered = 0_u64;
        let mut update = 0_u64;
        for &target in case.ride.points() {
            for step in 1..=steps_per_leg {
                hang_tick!(format!(
                    "{}: tempo step {step}/{steps_per_leg} toward {target}",
                    case.id
                ));
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

    #[kithara_test_utils::kithara::hang_watchdog(timeout = Duration::from_secs(5), ctx = String)]
    async fn capture_blocks(
        &mut self,
        case: SyncCase,
        capture_frames: usize,
        block_frames: usize,
        paced: bool,
    ) -> Vec<f32> {
        let capture_kind = if paced { "paced" } else { "product" };
        let observed_frame = SessionFrame::new(
            i64::try_from(self.output_frames).expect("actual capture frame fits session axis"),
        );
        self.capture_marks = self
            .decks
            .iter()
            .flat_map(|deck| deck.observe(|observation| observation.consumed.clone()))
            .map(|(item, slot)| (observed_frame, item, slot))
            .collect();
        #[cfg(not(target_os = "android"))]
        self.mark("capture start");
        let mut pcm = Vec::with_capacity(capture_frames * usize::from(CHANNELS));
        while pcm.len() < capture_frames * usize::from(CHANNELS) {
            let remaining_frames =
                (capture_frames * usize::from(CHANNELS) - pcm.len()) / usize::from(CHANNELS);
            hang_tick!(format!(
                "{}: capturing at frame {} with {remaining_frames} frames remaining",
                case.id, self.output_frames
            ));
            let frames = remaining_frames.min(block_frames);
            let started = paced.then(Instant::now);
            let block = self.render(case, frames).await;
            let observed_frame = SessionFrame::new(
                i64::try_from(self.output_frames).expect("actual capture frame fits session axis"),
            );
            self.capture_marks.extend(
                self.decks
                    .iter()
                    .flat_map(|deck| deck.observe(|observation| observation.consumed.clone()))
                    .map(|(item, slot)| (observed_frame, item, slot)),
            );
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
        #[cfg(not(target_os = "android"))]
        {
            self.mark("capture end");
            let (beats, downbeats) = kithara_integration_tests::cochlea::marked_rhythm_markers(
                &pcm,
                CHANNELS,
                case.sample_rate,
            );
            self.master_tap.evidence(
                "capture_markers",
                serde_json::json!({"beats": beats, "downbeats": downbeats}),
            );
            let observations: Vec<_> = self
                .decks
                .iter()
                .map(|deck| {
                    deck.observe(|observation| {
                        serde_json::json!({
                            "deck": u64::from(deck.id()),
                            "timeline": observation.timeline,
                            "tracks": format!("{:?}", observation.tracks),
                            "settled": format!("{:?}", observation.settled),
                            "sync_requests": format!("{:?}", observation.sync_requests),
                            "owner_inputs": observation.owner_inputs.iter().enumerate().map(|(index, input)| serde_json::json!({
                                "index": index, "item": input.item.map(u64::from), "at": input.at.map(i64::from),
                                "kind": format!("{:?}", input.kind), "trajectory": input.trajectory.as_ref().map(|host| format!("{host:?}")),
                            })).collect::<Vec<_>>(),
                            "speed_inputs": observation.speeds.iter().map(|speed| serde_json::json!({
                                "item": u64::from(speed.item), "seq": speed.seq.get(), "at": format!("{:?}", speed.at),
                                "input": speed.input, "host_input": speed.host_input, "planned": format!("{:?}", speed.planned),
                                "landing": format!("{:?}", speed.landing), "consumed": format!("{:?}", speed.consumed),
                            })).collect::<Vec<_>>(),
                            "cue_inputs": observation.cue_curves.iter().map(|cue| serde_json::json!({
                                "item": u64::from(cue.item), "seq": cue.seq.get(), "position_ns": cue.position.as_nanos().to_string(),
                                "host_input": cue.host_input, "consumed": format!("{:?}", cue.consumed),
                                "native_origin": cue.native.map(|origin| format!("{origin:?}")), "adopted_segment": cue.segment.map(kithara::signal::SegmentId::get),
                            })).collect::<Vec<_>>(),
                            "capture_facts": self.capture_marks.iter().filter(|(_, item, _)| observation.tracks.iter().any(|track| track.track.item == *item))
                                .map(|(frame, item, slot)| serde_json::json!({"frame": i64::from(*frame), "item": u64::from(*item), "slot": format!("{slot:?}")})).collect::<Vec<_>>(),
                        })
                    })
                })
                .collect();
            self.master_tap
                .evidence("observations", serde_json::json!(observations));
        }
        pcm
    }
}
