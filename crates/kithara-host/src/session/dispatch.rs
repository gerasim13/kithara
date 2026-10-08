use std::num::NonZeroU32;

use firewheel::{FirewheelContext, error::UpdateError};
use kithara_command::{Answer, Post, Seq};
use kithara_config::ConfigOwner;
use kithara_play::{PlayError, StreamShape};
use tracing::{debug, trace, warn};

use super::{
    protocol::{HostMailbox, SessionError, SessionSampleRate},
    state::SessionState,
    transport::{self, RouteRestartStatus},
};
use crate::{DeckId, HostOwner, HostSettled};

pub(crate) fn run_host_cmd<S, O: HostOwner<S>>(
    owner: &mut O,
    command: O::Command,
) -> Result<Option<Seq>, PlayError> {
    owner.apply(command)
}

/// Pending ticket answers belong to the owner, not to a second deck dispatcher.
pub(crate) struct OwnerPosts {
    pending: Vec<(Seq, Answer<PlayError>)>,
    closing: Vec<(DeckId, Answer<PlayError>)>,
}

impl OwnerPosts {
    pub(crate) fn new() -> Self {
        Self {
            pending: Vec::new(),
            closing: Vec::new(),
        }
    }
    pub(crate) fn drain<S, O: HostOwner<S>>(
        &mut self,
        owner: &mut O,
        mailbox: &mut HostMailbox<O::Command>,
    ) {
        let mut posts = Self::take_posts(mailbox).into_iter().peekable();
        while let Some(Post { command, answer }) = posts.next() {
            if O::is_next_tempo(&command)
                && posts
                    .peek()
                    .is_some_and(|post| O::is_next_tempo(&post.command))
            {
                answer.answer(Err(PlayError::Superseded));
                continue;
            }
            let releasing = O::release_id(&command);
            match run_host_cmd(owner, command) {
                Ok(_) if releasing.is_some() => {
                    if let Some(id) = releasing {
                        self.closing.push((id, answer));
                    }
                }
                Ok(Some(seq)) => self.pending.push((seq, answer)),
                outcome => answer.answer(outcome.map(|_| ())),
            }
        }
    }

    fn take_posts<C>(mailbox: &mut HostMailbox<C>) -> Vec<Post<C, PlayError>> {
        mailbox.drain().collect()
    }

    pub(crate) fn pass<S, O: HostOwner<S>>(&mut self, owner: &mut O) {
        let settled = owner.pass();
        if !self.pending.is_empty() || !self.closing.is_empty() {
            self.settle(settled);
        }
    }

    fn settle(&mut self, settled: Vec<HostSettled>) {
        for settled in settled {
            match settled {
                HostSettled::Replanned { from, to } => {
                    if let Some((seq, _)) = self.pending.iter_mut().find(|(seq, _)| *seq == from) {
                        *seq = to;
                    }
                }
                HostSettled::Settings { seq, outcome, .. }
                | HostSettled::Batch { seq, outcome } => {
                    if let Some(index) = self.pending.iter().position(|(held, _)| *held == seq) {
                        let (_, answer) = self.pending.remove(index);
                        answer.answer(outcome.map(|_| ()).map_err(|reason| match reason {
                            kithara_command::Rejection::Late => PlayError::Late,
                            kithara_command::Rejection::Stale => {
                                PlayError::Internal("owner batch basis is stale".into())
                            }
                            kithara_command::Rejection::Unanswered => PlayError::Closed,
                            kithara_command::Rejection::Refused(error) => error,
                        }));
                    }
                }
                HostSettled::Closed { deck } => {
                    let mut index = 0;
                    while index < self.closing.len() {
                        if self.closing[index].0 == deck {
                            let (_, answer) = self.closing.remove(index);
                            answer.answer(Ok(()));
                        } else {
                            index += 1;
                        }
                    }
                }
            }
        }
    }
}
fn measured_stream_shape<T, S>(state: &SessionState<T, S>) -> Option<StreamShape> {
    if state.stream_needs_restart {
        return None;
    }
    state
        .ctx
        .as_ref()
        .and_then(FirewheelContext::stream_info)
        .map(|info| StreamShape::new(info.max_block_frames, info.sample_rate))
}

pub(super) fn sample_rate<T, S>(state: &SessionState<T, S>) -> SessionSampleRate {
    let measured = measured_stream_shape(state).map(|shape| shape.sample_rate.get());
    SessionSampleRate::new(measured, state.settings.config().sample_rate().get())
}

pub(super) fn stream_shape<T, S>(state: &SessionState<T, S>) -> Option<StreamShape> {
    measured_stream_shape(state).or_else(|| {
        Some(StreamShape::new(
            state.requested_max_block_frames?,
            state.settings.config().sample_rate(),
        ))
    })
}

/// One pump of the session on its own interval: a deferred or dead stream
/// restarts, the graph updates, and the transport's commits and receipts
/// settle.
pub(crate) fn tick_session<T, S>(state: &mut SessionState<T, S>) -> Result<(), SessionError> {
    if state.stream_needs_restart {
        if let Err(err) = restart_stream(state) {
            warn!(?err, "[KITHARA-ROUTE] deferred stream restart failed");
            return Err(SessionError::RestartFailed {
                reason: "deferred stream restart".into(),
                r#source: err.to_string(),
            });
        }
        if state.stream_needs_restart {
            return Ok(());
        }
    }

    let update = state.ctx.as_mut().map(FirewheelContext::update);
    if let Some(Err(err)) = update {
        return Err(update_failed(&err));
    }
    if stream_died(state) {
        return restart_dead_stream(state);
    }
    transport::observe_commits(state);
    Ok(())
}

fn update_failed(err: &UpdateError) -> SessionError {
    warn!(?err, "[KITHARA-ROUTE] firewheel update failed");
    SessionError::Graph(format!("{err:?}"))
}

/// A context that went inactive under a session that believes its stream is
/// running lost that stream: Firewheel hands the processor back when it stops,
/// and since 0.14 that is the only place the death shows up — it is no longer
/// reported as an update error.
pub(super) fn stream_died<T, S>(state: &SessionState<T, S>) -> bool {
    state.stream.is_some()
        && !state.stream_needs_restart
        && state.ctx.as_ref().is_some_and(|ctx| !ctx.is_active())
}

fn restart_dead_stream<T, S>(state: &mut SessionState<T, S>) -> Result<(), SessionError> {
    state.stream_needs_restart = true;
    state.publish_root();
    warn!("session stream stopped unexpectedly; restarting audio stream");
    trace!(
        sample_rate = state.settings.config().sample_rate().get(),
        "[KITHARA-ROUTE] firewheel context went inactive under a live stream"
    );
    restart_stream(state).map_err(|restart_err| SessionError::RestartFailed {
        reason: "audio stream stopped".to_owned(),
        r#source: restart_err.to_string(),
    })
}

pub(crate) fn invalidate_audio_route<T, S>(
    state: &mut SessionState<T, S>,
    reason: &str,
) -> Result<(), SessionError> {
    debug!(
        reason,
        ctx_ready = state.ctx.is_some(),
        stream_needs_restart = state.stream_needs_restart,
        "[KITHARA-ROUTE] audio route invalidated"
    );
    if state.ctx.is_none() {
        return Ok(());
    }
    state.stream_needs_restart = true;
    restart_stream(state).map_err(|err| SessionError::RestartFailed {
        reason: reason.to_owned(),
        r#source: err.to_string(),
    })
}

/// Restarts the output at the rate the settings ask for.
pub(super) fn restart_stream<T, S>(state: &mut SessionState<T, S>) -> Result<(), SessionError> {
    if state.ctx.is_none() {
        return Err(SessionError::NoContext);
    }
    let sample_rate = state.settings.config().sample_rate().get();
    debug!(sample_rate, "[KITHARA-ROUTE] restarting firewheel stream");
    if transport::prepare_route_restart(state)? == RouteRestartStatus::Pending {
        trace!("[KITHARA-ROUTE] waiting for the previous stream processor to stop");
        return Ok(());
    }
    let fw_ctx = state.ctx.as_mut().ok_or(SessionError::NoContext)?;
    let stream = (state.start_stream_fn)(fw_ctx, sample_rate).map_err(SessionError::StreamStart)?;
    state.stream = Some(stream);
    state.reserved_session_grid = None;
    state.stream_needs_restart = false;
    state.publish_root();
    trace_stream_info(state, "restart-stream");
    debug!(
        sample_rate,
        "[KITHARA-ROUTE] firewheel stream restart complete"
    );
    Ok(())
}

pub(super) fn trace_stream_info<T, S>(state: &SessionState<T, S>, context: &'static str) {
    if let Some(info) = state.ctx.as_ref().and_then(FirewheelContext::stream_info) {
        trace!(
            context,
            sample_rate = info.sample_rate.get(),
            prev_sample_rate = info.prev_sample_rate.get(),
            max_block_frames = info.max_block_frames.get(),
            out_channels = info.num_stream_out_channels,
            stream_needs_restart = state.stream_needs_restart,
            "[KITHARA-ROUTE] session stream-info"
        );
    } else {
        trace!(
            context,
            sample_rate = state.settings.config().sample_rate().get(),
            requested_max_block_frames = state.requested_max_block_frames.map(NonZeroU32::get),
            stream_needs_restart = state.stream_needs_restart,
            "[KITHARA-ROUTE] session stream-info unavailable"
        );
    }
}

#[cfg(test)]
mod tests {
    use std::{
        num::{NonZeroU32, NonZeroUsize},
        sync::atomic::AtomicBool,
    };

    use audioadapter_buffers::direct::InterleavedSlice;
    use firewheel::{
        ActivateInfo,
        backend::BackendProcessInfo,
        channel_config::{ChannelConfig, ChannelCount},
        node::{
            AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, EmptyConfig,
            NodeError, ProcBuffers, ProcExtra, ProcInfo, ProcessStatus, StreamStatus,
        },
        processor::FirewheelProcessor,
    };
    use kithara_command::{When, mailbox};
    use kithara_config::{Config, ConfigOwner};
    use kithara_events::EventBus;
    use kithara_output::OutputGroup;
    use kithara_platform::{
        sync::{
            Arc,
            atomic::{AtomicU64, AtomicUsize, Ordering},
        },
        time::Duration,
    };
    use kithara_play::{BufferGeometryError, DeckMixerConfig, PlayError, Tempo};
    use kithara_test_utils::{
        bufpool::{TestPools, pools},
        kithara,
    };
    use kithara_warp::{BeatGridSnapshot, BeatGridState, BeatGridUnavailable, MapAxis};
    use ringbuf::{HeapRb, traits::Split};

    use super::*;
    use crate::{
        api::{SessionDuckingMode, Tap},
        bridge::MixTapWriter,
        host::HostSettingsChange,
        rt::MetronomeConfigChange,
        session::{
            applied_spans,
            protocol::SessionError,
            state::{Deck, SessionState, TapSlot, add_graph_node},
            tests::{
                graph::{ask, attach, state as test_state},
                ring::{MasterRing, RingBackend, RingBackendConfig, RingLayout},
            },
        },
    };

    #[derive(Default)]
    struct RouteLossProbe {
        fail_next_start: AtomicBool,
        start_count: AtomicUsize,
    }

    impl RouteLossProbe {
        fn reset(&self) {
            self.start_count.store(0, Ordering::SeqCst);
            self.fail_next_start.store(false, Ordering::SeqCst);
        }
    }

    thread_local! {
        static ROUTE_LOSS: RouteLossProbe = RouteLossProbe::default();
    }

    fn route_loss<R>(f: impl FnOnce(&RouteLossProbe) -> R) -> R {
        ROUTE_LOSS.with(f)
    }

    /// The fixture stream. Holding the processor is the whole of it: dropping
    /// this is what a lost audio stream looks like to the context, so a test
    /// simulates the loss by dropping `state.stream`.
    struct RouteLossStream {
        processor: FirewheelProcessor,
        block_frames: usize,
    }

    impl RouteLossStream {
        /// The interleaved stereo block the graph renders at `clock_samples`.
        fn render(&mut self, clock_samples: u64) -> Vec<f32> {
            let mut block = vec![0.0; self.block_frames * 2];
            let input = InterleavedSlice::new(&[] as &[f32], 0, 0).expect("an empty input");
            let mut output = InterleavedSlice::new_mut(&mut block, 2, self.block_frames)
                .expect("a stereo block");
            self.processor.process(
                &input,
                &mut output,
                BackendProcessInfo {
                    frames: self.block_frames,
                    process_timestamp: Some(bevy_platform::time::Instant::now()),
                    duration_since_stream_start: Duration::from_secs_f64(
                        f64::from(
                            u32::try_from(clock_samples).expect("the fixture clock fits u32"),
                        ) / f64::from(TestState::DEFAULT_SAMPLE_RATE),
                    ),
                    input_stream_status: StreamStatus::empty(),
                    output_stream_status: StreamStatus::empty(),
                    dropped_frames: 0,
                    process_to_playback_delay: None,
                },
            );
            block
        }
    }

    type TestState = SessionState<RouteLossStream, TestPools>;

    /// The left channel of `blocks` blocks the stream renders from `clock` on.
    fn render_left(state: &mut TestState, clock: &mut u64, blocks: usize) -> Vec<f32> {
        let stream = state.stream.as_mut().expect("the stream runs");
        let frames = u64::try_from(stream.block_frames).expect("a block fits the clock");
        let mut left = Vec::new();
        for _ in 0..blocks {
            left.extend(stream.render(*clock).into_iter().step_by(2));
            *clock += frames;
        }
        left
    }

    /// A stereo source holding every sample at its value.
    #[derive(Clone, Copy)]
    struct DcNode(f32);

    impl AudioNode for DcNode {
        type Configuration = EmptyConfig;

        fn construct_processor(
            &self,
            _config: &Self::Configuration,
            _cx: ConstructProcessorContext,
        ) -> Result<impl AudioNodeProcessor, NodeError> {
            Ok(*self)
        }

        fn info(&self, _config: &Self::Configuration) -> Result<AudioNodeInfo, NodeError> {
            Ok(AudioNodeInfo::new()
                .debug_name("dc")
                .channel_config(ChannelConfig {
                    num_inputs: ChannelCount::ZERO,
                    num_outputs: ChannelCount::STEREO,
                }))
        }
    }

    impl AudioNodeProcessor for DcNode {
        fn process(
            &mut self,
            info: &ProcInfo,
            buffers: ProcBuffers,
            _extra: &mut ProcExtra,
        ) -> ProcessStatus {
            for output in &mut *buffers.outputs {
                output[..info.frames].fill(self.0);
            }
            ProcessStatus::OutputsModified
        }
    }

    #[derive(Debug, thiserror::Error)]
    #[error("route lost")]
    struct RouteLossError;

    fn start_route_loss_stream(
        ctx: &mut FirewheelContext,
        sample_rate: u32,
    ) -> Result<RouteLossStream, String> {
        route_loss(|probe| probe.start_count.fetch_add(1, Ordering::SeqCst));
        if route_loss(|probe| probe.fail_next_start.swap(false, Ordering::SeqCst)) {
            return Err(RouteLossError.to_string());
        }
        let sample_rate = NonZeroU32::new(sample_rate).unwrap_or(
            NonZeroU32::new(TestState::DEFAULT_SAMPLE_RATE)
                .expect("invariant: fixture default sample rate is non-zero"),
        );
        let max_block_frames =
            NonZeroU32::new(512).expect("invariant: fixture block size is non-zero");
        let processor = ctx
            .activate(ActivateInfo {
                sample_rate,
                max_block_frames,
                num_stream_in_channels: 0,
                num_stream_out_channels: 2,
                input_to_output_latency_seconds: 0.0,
            })
            .map_err(|err| err.to_string())?;
        Ok(RouteLossStream {
            processor,
            block_frames: max_block_frames.get() as usize,
        })
    }

    fn registration(grid_id: BeatGridId) -> DeckRegistration<TestPools> {
        let mut registration = DeckRegistration::new(
            grid_id,
            EventBus::default(),
            pools(),
            DeckMixerConfig::default(),
        );
        registration.response_budget_frames = NonZeroUsize::new(448);
        registration
    }

    /// Attaches a deck, which the session registers and starts.
    fn insert(state: &mut TestState) -> BeatGridId {
        let grid_id = BeatGridId::allocate().expect("fixture player grid id");
        match ask(state, attach(registration(grid_id))) {
            Ok(_) => grid_id,
            Err(err) => panic!("the deck failed to start: {err}"),
        }
    }

    /// Stops the deck `grid_id` and removes it from the session.
    fn remove(state: &mut TestState, grid_id: BeatGridId) {
        assert!(matches!(ask(state, HostCmd::Detach { grid_id }), Ok(())));
    }

    /// Asks the session for `rate` from the next block on.
    fn configure_sample_rate(state: &mut TestState, rate: u32) {
        let rate = NonZeroU32::new(rate).expect("a fixture rate is not zero");
        assert!(matches!(
            ask(
                state,
                HostCmd::Configure {
                    change: HostSettingsChange::SampleRate(rate),
                    at: When::Next,
                }
            ),
            Ok(())
        ));
    }

    /// Moves the session's output to a new platform route.
    fn change_route_to(state: &mut TestState, reason: &str) {
        assert!(matches!(
            ask(
                state,
                HostCmd::InvalidateAudioRoute {
                    reason: reason.to_owned(),
                }
            ),
            Ok(())
        ));
    }

    fn deck(state: &TestState, index: usize) -> &Deck<TestPools> {
        state
            .graph
            .deck(index)
            .expect("the registered deck is present under the host")
    }

    fn deck_count(state: &TestState) -> usize {
        state.graph.len()
    }

    fn host_grid(state: &TestState) -> BeatGridSnapshot {
        state.root.grid().clone()
    }

    fn assert_route_boundary(before: &BeatGridSnapshot, boundary: &BeatGridSnapshot) {
        assert_eq!(
            boundary.state(),
            BeatGridState::Unavailable(BeatGridUnavailable::NoGeometry)
        );
        assert!(boundary.revision() > before.revision());
        let MapAxis::Session(before_axis) = before.axis() else {
            panic!("the previous host grid uses the session axis")
        };
        let MapAxis::Session(boundary_axis) = boundary.axis() else {
            panic!("the route boundary uses the session axis")
        };
        assert!(boundary_axis.epoch() > before_axis.epoch());
    }

    fn deck_by_grid(state: &TestState, grid_id: BeatGridId) -> &Deck<TestPools> {
        let index = state
            .graph
            .index_by_grid(grid_id)
            .expect("the deck is registered");
        state
            .graph
            .deck(index)
            .expect("the registered deck is present")
    }

    #[kithara::test]
    fn an_attached_deck_runs_until_it_is_detached() {
        route_loss(RouteLossProbe::reset);
        let mut state = test_state(start_route_loss_stream);
        let host_id = state.root.id();

        let grid_id = insert(&mut state);
        assert!(state.root.holds(grid_id));
        assert!(state.root_view.holds(grid_id));
        assert!(deck_by_grid(&state, grid_id).started());

        remove(&mut state, grid_id);

        assert_eq!(state.root.id(), host_id);
        assert!(!state.root.holds(grid_id));
        assert!(!state.root_view.holds(grid_id));
        assert_eq!(deck_count(&state), 0);
    }

    #[kithara::test]
    fn attach_refuses_an_identity_the_session_already_holds() {
        route_loss(RouteLossProbe::reset);
        let mut state = test_state(start_route_loss_stream);
        let grid_id = insert(&mut state);
        let next_player_id = state.next_player_id;

        assert!(matches!(
            ask(&mut state, attach(registration(grid_id))),
            Err(PlayError::Session(SessionError::DeckAttached(refused)))
                if refused == grid_id
        ));
        assert_eq!(state.next_player_id, next_player_id);
        assert!(state.root_view.holds(grid_id));
        assert_eq!(deck_count(&state), 1);
    }

    #[kithara::test]
    fn detach_refuses_a_deck_the_session_does_not_hold() {
        route_loss(RouteLossProbe::reset);
        let mut state = test_state(start_route_loss_stream);
        let held = insert(&mut state);
        let grid_id = BeatGridId::allocate().expect("fixture foreign grid id");

        assert!(matches!(
            ask(&mut state, HostCmd::Detach { grid_id }),
            Err(PlayError::Session(SessionError::DeckNotFound(refused)))
                if refused == grid_id
        ));
        assert!(state.root_view.holds(held));
    }

    #[kithara::test]
    fn root_view_publishes_the_decks_the_session_holds() {
        route_loss(RouteLossProbe::reset);
        let mut state = test_state(start_route_loss_stream);
        assert!(state.root_view.is_empty());
        let grid_id = insert(&mut state);

        assert!(state.root_view.holds(grid_id));
        assert!(!state.root_view.is_empty());

        remove(&mut state, grid_id);
        assert!(!state.root_view.holds(grid_id));
        assert!(state.root_view.is_empty());
    }

    #[kithara::test]
    fn exhausted_player_identity_refuses_the_deck_whole() {
        let mut state = test_state(start_route_loss_stream);
        let grid_id = BeatGridId::allocate().expect("fixture player grid id");
        state.next_player_id = u64::MAX;

        let reply = ask(&mut state, attach(registration(grid_id)));

        assert!(matches!(
            reply,
            Err(PlayError::Session(SessionError::PlayerIdExhausted))
        ));
        assert_eq!(state.next_player_id, u64::MAX);
        assert_eq!(deck_count(&state), 0);
        assert!(!state.root.holds(grid_id));
        assert!(!state.root_view.holds(grid_id));
        assert!(state.reserved_session_grid.is_some());
    }

    #[kithara::test]
    fn the_published_sample_rate_separates_the_measured_stream_from_the_request() {
        route_loss(RouteLossProbe::reset);

        let mut state = test_state(start_route_loss_stream);
        let before = state.root_view.sample_rate();
        assert_eq!(
            before.measured, None,
            "a session with no stream has measured nothing"
        );
        assert_eq!(
            before.output(),
            TestState::DEFAULT_SAMPLE_RATE,
            "until a stream exists the resampler is built for the requested rate"
        );

        configure_sample_rate(&mut state, 48_000);
        assert!(matches!(
            state.root_view.sample_rate(),
            SessionSampleRate {
                measured: None,
                requested: 48_000,
                ..
            }
        ));
        insert(&mut state);
        assert!(matches!(
            state.root_view.sample_rate(),
            SessionSampleRate {
                measured: Some(48_000),
                requested: 48_000,
                ..
            }
        ));
    }

    #[kithara::test]
    fn a_sample_rate_set_while_idle_is_the_rate_play_starts_the_stream_at() {
        route_loss(RouteLossProbe::reset);

        let mut state = test_state(start_route_loss_stream);
        let rate = NonZeroU32::new(48_000).expect("48000 is not zero");
        configure_sample_rate(&mut state, rate.get());
        insert(&mut state);

        let started = state
            .ctx
            .as_ref()
            .and_then(FirewheelContext::stream_info)
            .expect("play starts the stream")
            .sample_rate;
        assert_eq!(
            started, rate,
            "the stream starts at the rate set while idle"
        );
    }

    #[kithara::test]
    fn the_published_stream_shape_prefers_measurement_over_an_explicit_request() {
        route_loss(RouteLossProbe::reset);

        let mut state = test_state(start_route_loss_stream);
        assert_eq!(state.root_view.output.get().stream_shape, None);

        state.requested_max_block_frames = NonZeroU32::new(128);
        state.publish_root();
        let requested = state
            .root_view
            .output
            .get()
            .stream_shape
            .expect("the explicit output block is published before stream start");
        assert_eq!(requested.max_block_frames.get(), 128);
        assert_eq!(requested.sample_rate.get(), TestState::DEFAULT_SAMPLE_RATE);

        let player_id = insert(&mut state);
        let measured = state
            .root_view
            .output
            .get()
            .stream_shape
            .expect("the running stream publishes its measured output shape");
        assert_eq!(measured.max_block_frames.get(), 512);
        assert_eq!(measured.sample_rate.get(), TestState::DEFAULT_SAMPLE_RATE);
        configure_sample_rate(&mut state, 48_000);
        assert_eq!(
            state
                .root_view
                .output
                .get()
                .stream_shape
                .expect("published shape")
                .sample_rate
                .get(),
            48_000
        );
        remove(&mut state, player_id);
        let stopped = state
            .root_view
            .output
            .get()
            .stream_shape
            .expect("configured shape after stop");
        assert_eq!(stopped.max_block_frames.get(), 128);
        assert_eq!(stopped.sample_rate.get(), 48_000);
    }

    #[kithara::test]
    fn a_deck_whose_buffers_outgrow_the_measured_block_is_refused_whole() {
        route_loss(RouteLossProbe::reset);

        let mut state = test_state(start_route_loss_stream);
        state.requested_max_block_frames = NonZeroU32::new(128);
        let grid_id = BeatGridId::allocate().expect("fixture player grid id");
        let mut registration = registration(grid_id);
        registration.render_quantum_frames = NonZeroUsize::new(64);
        registration.response_budget_frames = NonZeroUsize::new(441);

        assert!(matches!(
            ask(&mut state, attach(registration)),
            Err(PlayError::Session(SessionError::BufferGeometry(
                BufferGeometryError::BudgetExceeded {
                    max_block_frames: 512,
                    render_quantum_frames: 64,
                    required_frames: 639,
                    budget_frames: 441,
                }
            )))
        ));
        assert_eq!(deck_count(&state), 0);
        assert!(!state.root.holds(grid_id));
        assert!(
            state.ctx.is_none(),
            "a refused deck leaves no output running"
        );
    }

    #[kithara::test]
    fn explicit_audio_route_invalidation_restarts_stream_without_backend_error() {
        route_loss(RouteLossProbe::reset);

        let mut state = test_state(start_route_loss_stream);
        insert(&mut state);
        assert!(matches!(
            state.root_view.sample_rate(),
            SessionSampleRate {
                measured: Some(44_100),
                requested: 44_100,
                ..
            }
        ));
        let slot_node = deck(&state, 0)
            .slot_node
            .expect("a started deck has its slot node");
        assert_eq!(
            route_loss(|probe| probe.start_count.load(Ordering::SeqCst)),
            1
        );
        let before_route = host_grid(&state);

        change_route_to(&mut state, "oldDeviceUnavailable");

        assert_eq!(
            route_loss(|probe| probe.start_count.load(Ordering::SeqCst)),
            2,
            "explicit platform route invalidation must restart the audio stream"
        );
        assert_route_boundary(&before_route, &host_grid(&state));
        let first_boundary = host_grid(&state);
        change_route_to(&mut state, "newDeviceAvailable");
        assert_eq!(
            route_loss(|probe| probe.start_count.load(Ordering::SeqCst)),
            3,
            "a second physical route invalidation must start a new stream generation"
        );
        assert_route_boundary(&first_boundary, &host_grid(&state));
        assert!(
            state.ctx.is_some(),
            "route invalidation must keep the graph context"
        );
        assert!(
            deck(&state, 0).started(),
            "route invalidation must keep the player graph logically started"
        );
        assert!(
            state
                .ctx
                .as_ref()
                .is_some_and(|ctx| ctx.contains_node(slot_node)),
            "route invalidation must keep the deck's slot node in the graph"
        );
        assert_eq!(deck(&state, 0).slot_node, Some(slot_node));
        assert!(!state.stream_needs_restart);
    }

    #[kithara::test]
    fn unexpected_stream_stop_restarts_stream_without_dropping_the_deck_slot() {
        route_loss(RouteLossProbe::reset);

        let mut state = test_state(start_route_loss_stream);
        insert(&mut state);
        assert!(state.ctx.is_some());
        assert!(deck(&state, 0).started());
        assert_eq!(
            route_loss(|probe| probe.start_count.load(Ordering::SeqCst)),
            1
        );
        let slot_node = deck(&state, 0)
            .slot_node
            .expect("a started deck has its slot node");
        let before_route = host_grid(&state);

        state.stream = None;
        assert!(tick_session(&mut state).is_ok());

        assert_eq!(
            route_loss(|probe| probe.start_count.load(Ordering::SeqCst)),
            2,
            "stream loss must restart the audio stream immediately"
        );
        assert_route_boundary(&before_route, &host_grid(&state));
        assert!(
            state.ctx.is_some(),
            "session must keep the graph context across stream restart"
        );
        assert!(
            state.session_output_node_id.is_some(),
            "session output node id must survive stream restart"
        );
        assert!(
            deck(&state, 0).started(),
            "player graph must remain logically started after stream restart"
        );
        assert!(
            state
                .ctx
                .as_ref()
                .is_some_and(|ctx| ctx.contains_node(slot_node)),
            "the deck's slot node must stay in the graph across stream restart"
        );
        assert_eq!(deck(&state, 0).slot_node, Some(slot_node));
        assert!(!state.stream_needs_restart);
    }

    #[kithara::test]
    fn stream_loss_seen_while_draining_host_commands_restarts_the_stream() {
        route_loss(RouteLossProbe::reset);

        let mut state = test_state(start_route_loss_stream);
        insert(&mut state);
        assert_eq!(
            route_loss(|probe| probe.start_count.load(Ordering::SeqCst)),
            1
        );

        let (_postbox, mut mailbox) = mailbox::<HostCmd<TestPools>, PlayError>();

        state.stream = None;
        drain_host_posts(&mut state, &mut mailbox, |_| {});

        assert_eq!(
            route_loss(|probe| probe.start_count.load(Ordering::SeqCst)),
            2,
            "a stream drop observed during a host command drain must restart the stream"
        );
        assert!(!state.stream_needs_restart);
    }

    #[kithara::test]
    fn empty_host_drain_publishes_a_rendered_transport_commit() {
        let sample_rate = NonZeroU32::new(48_000).expect("test sample rate");
        let (writer, mut reader) = MasterRing::open(512, 4);
        let mut writer = Some(writer);
        let mut state: SessionState<RingBackend, TestPools> = test_state(move |ctx, _| {
            RingBackend::start(
                ctx,
                RingBackendConfig::new(
                    sample_rate,
                    RingLayout::Stereo,
                    writer.take().ok_or("backend already started")?,
                ),
            )
            .map_err(|err| err.to_string())
        });
        crate::session::state::ensure_ctx(&mut state).expect("active browser graph");
        let stream = state.stream.as_mut().expect("stream started");
        stream.arm();
        stream.render_block(0).expect("initial render");
        let _ = reader.drain(512);
        assert_eq!(
            state.root_view.grid().state(),
            BeatGridState::Unavailable(BeatGridUnavailable::NoGeometry)
        );

        let tempo = Tempo::new(90.0).expect("valid tempo");
        assert!(
            state
                .exec(HostSettingsChange::Tempo(tempo), When::Next, &mut ())
                .is_ok()
        );
        assert!(tick_session(&mut state).is_ok());
        for clock_samples in [512, 1024, 1536] {
            state
                .stream
                .as_mut()
                .expect("active stream")
                .render_block(clock_samples)
                .expect("changed graph renders");
            let _ = reader.drain(512);
        }
        let before = state.root_view.grid();
        let (_postbox, mut mailbox) = mailbox::<HostCmd<TestPools>, PlayError>();

        drain_host_posts(&mut state, &mut mailbox, |_| {});

        assert!(state.root_view.grid().revision() > before.revision());
    }

    #[kithara::test]
    fn failed_stream_restart_is_retried_on_next_tick() {
        route_loss(RouteLossProbe::reset);

        let mut state = test_state(start_route_loss_stream);
        insert(&mut state);
        assert_eq!(
            route_loss(|probe| probe.start_count.load(Ordering::SeqCst)),
            1
        );
        let before_route = host_grid(&state);

        state.stream = None;
        route_loss(|probe| probe.fail_next_start.store(true, Ordering::SeqCst));
        match tick_session(&mut state) {
            Err(err) => assert!(
                matches!(err, SessionError::RestartFailed { .. }),
                "restart failure must be surfaced, got {err:?}"
            ),
            Ok(()) => panic!("failed restart must return an error"),
        }

        assert!(
            state.stream_needs_restart,
            "a failed restart must leave retry state armed"
        );
        assert_eq!(
            route_loss(|probe| probe.start_count.load(Ordering::SeqCst)),
            2
        );
        let boundary = host_grid(&state);
        assert_route_boundary(&before_route, &boundary);

        assert!(tick_session(&mut state).is_ok());
        assert_eq!(
            route_loss(|probe| probe.start_count.load(Ordering::SeqCst)),
            3,
            "next tick must retry the stream restart"
        );
        let retried = host_grid(&state);
        assert_eq!(retried.stamp(), boundary.stamp());
        assert_eq!(retried.axis(), boundary.axis());
        assert!(!state.stream_needs_restart);
        assert!(deck(&state, 0).started());
    }

    fn mix_tap_writer(drops: &Arc<AtomicU64>) -> MixTapWriter {
        const TAP_CAPACITY: usize = 1_024;

        let (pcm, _cons) = HeapRb::<f32>::new(TAP_CAPACITY).split();
        MixTapWriter::new(pcm, Arc::clone(drops))
    }

    #[kithara::test]
    fn each_tap_takes_one_group_and_idle_teardown_clears_both() {
        route_loss(RouteLossProbe::reset);

        let mut state = test_state(start_route_loss_stream);
        let id = insert(&mut state);

        let drops = Arc::new(AtomicU64::new(0));
        let mut outputs = OutputGroup::new();
        outputs.push(mix_tap_writer(&drops));
        outputs.push(mix_tap_writer(&drops));
        assert!(matches!(
            ask(
                &mut state,
                HostCmd::AttachOutputs {
                    tap: Tap::Master,
                    outputs,
                }
            ),
            Ok(())
        ));
        assert!(
            matches!(state.taps.slot(Tap::Master), Some(TapSlot::Installed(_))),
            "a tap armed on a running session reaches the graph at once"
        );

        let mut second = OutputGroup::new();
        second.push(mix_tap_writer(&drops));
        assert!(
            matches!(
                ask(
                    &mut state,
                    HostCmd::AttachOutputs {
                        tap: Tap::Master,
                        outputs: second,
                    }
                ),
                Err(PlayError::Session(SessionError::TapActive))
            ),
            "a second consumer must be rejected instead of silently replacing the first"
        );

        let mut beside = OutputGroup::new();
        beside.push(mix_tap_writer(&drops));
        assert!(
            matches!(
                ask(
                    &mut state,
                    HostCmd::AttachOutputs {
                        tap: Tap::Output,
                        outputs: beside,
                    }
                ),
                Ok(())
            ),
            "the output tap takes its own group beside the master tap"
        );

        remove(&mut state, id);
        assert!(state.session_limiter_node_id.is_none());
        assert!(
            state.taps.slot(Tap::Master).is_none() && state.taps.slot(Tap::Output).is_none(),
            "idle teardown must clear both taps with the context they lived in"
        );
    }

    #[kithara::test]
    fn a_metronome_change_in_flight_at_an_idle_teardown_sounds_in_the_next_stream() {
        route_loss(RouteLossProbe::reset);

        let mut state = test_state(start_route_loss_stream);
        let id = insert(&mut state);
        assert!(matches!(
            ask(
                &mut state,
                HostCmd::Configure {
                    change: HostSettingsChange::Metronome(MetronomeConfigChange::Enabled(true)),
                    at: When::Next,
                }
            ),
            Ok(())
        ));
        assert!(
            !state.settings.config().metronome().enabled(),
            "the change waits for a block that never renders"
        );

        remove(&mut state, id);
        assert!(
            state.settings.config().metronome().enabled(),
            "the teardown folds the change in flight into the settings"
        );

        insert(&mut state);
        let block = state
            .stream
            .as_mut()
            .expect("the next stream runs")
            .render(0);
        assert!(
            block.iter().any(|sample| sample.abs() > 0.1),
            "the first block of the next stream clicks session beat 0"
        );
    }

    /// Sends `change` for the next block.
    fn configure_next(state: &mut TestState, change: HostSettingsChange) -> Result<(), PlayError> {
        ask(
            state,
            HostCmd::Configure {
                change,
                at: When::Next,
            },
        )
    }

    /// The Host queue's capacity: the batches in flight before a block
    /// answers them.
    fn host_queue_capacity() -> u16 {
        let capacity = kithara_command::ChannelConfig::builder()
            .build()
            .values()
            .capacity
            .get();
        u16::try_from(capacity).expect("the queue capacity fits a step count")
    }

    /// A metronome level of its own for every step of a run.
    fn level_change(step: u16) -> (f32, HostSettingsChange) {
        let level = 0.25 + f32::from(step) / 1024.0;
        (
            level,
            HostSettingsChange::Metronome(MetronomeConfigChange::Level(level)),
        )
    }

    #[kithara::test]
    fn two_restarts_with_a_change_between_them_leave_the_render_copy_on_the_host_settings() {
        route_loss(RouteLossProbe::reset);

        let mut state = test_state(start_route_loss_stream);
        insert(&mut state);
        configure_sample_rate(&mut state, 48_000);
        let enable = HostSettingsChange::Metronome(MetronomeConfigChange::Enabled(true));
        assert!(matches!(configure_next(&mut state, enable), Ok(())));
        let mut clock = 0;
        render_left(&mut state, &mut clock, 1);

        route_loss(|probe| probe.fail_next_start.store(true, Ordering::SeqCst));
        state.stream = None;
        assert!(matches!(
            tick_session(&mut state),
            Err(SessionError::RestartFailed { .. })
        ));

        let host = *state.settings.config();
        assert_eq!(
            state
                .ctx
                .as_mut()
                .and_then(FirewheelContext::proc_store_mut)
                .and_then(|store| applied_spans(store, 1)?.last())
                .map(|(_, span)| span.settings()),
            Some(host),
            "the restart seeds the render copy with the settings the Host reads"
        );
        assert_eq!(host.sample_rate().get(), 48_000, "the first restart's rate");
        assert!(
            host.metronome().enabled(),
            "the change the first stream applied is settled before the seed"
        );
    }

    #[kithara::test]
    fn changes_past_the_queue_capacity_flow_while_blocks_render_without_a_tick() {
        route_loss(RouteLossProbe::reset);

        let mut state = test_state(start_route_loss_stream);
        insert(&mut state);
        let mut clock = 0;
        let mut applied = state.settings.config().metronome().level();
        for step in 0..2 * host_queue_capacity() {
            let (level, change) = level_change(step);
            assert!(
                matches!(configure_next(&mut state, change), Ok(())),
                "change {step} goes out"
            );
            assert_eq!(
                state.settings.config().metronome().level(),
                applied,
                "the Host reads the change the last block applied"
            );
            render_left(&mut state, &mut clock, 1);
            applied = level;
        }
    }

    #[kithara::test]
    fn a_full_queue_refuses_a_change_until_a_block_answers_the_ones_in_flight() {
        route_loss(RouteLossProbe::reset);

        let mut state = test_state(start_route_loss_stream);
        insert(&mut state);
        let capacity = host_queue_capacity();
        for step in 0..capacity {
            assert!(
                matches!(configure_next(&mut state, level_change(step).1), Ok(())),
                "change {step} fits the queue"
            );
        }
        let before = *state.settings.config();

        assert!(matches!(
            configure_next(&mut state, level_change(capacity).1),
            Err(PlayError::Session(SessionError::HostQueueFull))
        ));
        assert_eq!(
            *state.settings.config(),
            before,
            "a refused change leaves the settings alone"
        );
        assert_eq!(
            state.settings.pending().count(),
            usize::from(capacity),
            "a refused change leaves the changes in flight alone"
        );

        let mut clock = 0;
        render_left(&mut state, &mut clock, 1);
        assert!(matches!(
            configure_next(&mut state, level_change(capacity).1),
            Ok(())
        ));
        assert_eq!(
            state.settings.config().metronome().level(),
            level_change(capacity - 1).0,
            "the block applied every change in flight"
        );
    }

    #[kithara::test]
    fn a_ducking_change_lowers_a_sounding_dc_along_a_ramp() {
        const DC: f32 = 0.25;
        /// The share of the session output `Hard` ducking leaves: 28 dB down.
        const HARD_DUCKED: f32 = 0.04;
        route_loss(RouteLossProbe::reset);

        let mut state = test_state(start_route_loss_stream);
        insert(&mut state);
        let session_output = state
            .session_output_node_id
            .expect("the session output runs");
        let ctx = state.ctx.as_mut().expect("the context runs");
        let dc = add_graph_node(ctx, DcNode(DC)).expect("the graph takes the source");
        ctx.connect(dc, session_output, &[(0, 0), (1, 1)], false)
            .expect("the source feeds the session output");
        ctx.update().expect("the graph takes the source in");
        let mut clock = 0;
        let before = render_left(&mut state, &mut clock, 8);
        let undiminished = *before.last().expect("the stream rendered");
        assert!(
            (undiminished - DC).abs() < 1e-4,
            "the DC reaches the output whole before the change: {undiminished}"
        );

        assert!(matches!(
            ask(
                &mut state,
                HostCmd::Configure {
                    change: HostSettingsChange::Ducking(SessionDuckingMode::Hard),
                    at: When::Next,
                }
            ),
            Ok(())
        ));
        let after = render_left(&mut state, &mut clock, 40);

        let ducked = DC * HARD_DUCKED;
        let settled = *after.last().expect("the stream rendered");
        assert!(
            (settled - ducked).abs() < 1e-4,
            "the DC settles at the hard ducking: {settled}, not {ducked}"
        );
        let steepest = [undiminished]
            .iter()
            .chain(&after)
            .zip(&after)
            .map(|(previous, sample)| (sample - previous).abs())
            .fold(0.0, f32::max);
        assert!(
            steepest < (DC - ducked) / 100.0,
            "no step between neighbouring samples on the way down: {steepest}"
        );
    }

    #[kithara::test]
    fn a_deck_attached_after_an_idle_teardown_leaves_through_the_next_one() {
        route_loss(RouteLossProbe::reset);

        let mut state = test_state(start_route_loss_stream);
        let first = insert(&mut state);
        remove(&mut state, first);

        let second = insert(&mut state);
        match ask(&mut state, HostCmd::Detach { grid_id: second }) {
            Ok(()) => {}
            Err(error) => {
                panic!("a deck that joined after a route boundary must follow the next: {error}")
            }
        }
    }

    #[kithara::test]
    fn session_output_has_exactly_one_limiter_rebuilt_on_route_recreate() {
        route_loss(RouteLossProbe::reset);

        let mut state = test_state(start_route_loss_stream);
        let id = insert(&mut state);
        assert!(
            state.session_limiter_node_id.is_some(),
            "limiter node exists after start"
        );

        remove(&mut state, id);
        assert!(state.session_limiter_node_id.is_none());
        assert!(state.session_output_node_id.is_none());

        insert(&mut state);
        assert!(
            state.session_limiter_node_id.is_some(),
            "route recreate rebuilds the limiter node"
        );
    }
}
