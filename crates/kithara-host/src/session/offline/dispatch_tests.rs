use std::{
    num::{NonZeroU32, NonZeroUsize},
    sync::atomic::AtomicBool,
};

use firewheel::{
    FirewheelContext,
    channel_config::{ChannelConfig, ChannelCount},
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, EmptyConfig,
        NodeError, ProcBuffers, ProcExtra, ProcInfo, ProcessStatus,
    },
};
use kithara_command::{When, mailbox};
use kithara_config::{Config, ConfigOwner};
use kithara_output::OutputGroup;
use kithara_platform::{
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};
use kithara_play::{
    BufferGeometryError, PlayError, PlayWorker, PlayWorkerConfig, ResourcePrep, Tempo,
};
use kithara_queue::{Queue, QueueConfig};
use kithara_test_utils::{
    bufpool::{TestPools, pools},
    kithara,
};
use kithara_warp::{BeatGridId, BeatGridSnapshot, BeatGridState, BeatGridUnavailable, MapAxis};
use ringbuf::{HeapRb, traits::Split};

use super::backend::{BackendConfig, OfflineStream};
use crate::{
    HostCommand, HostCore, HostOwner, HostSettingsExec,
    api::{SessionDuckingMode, Tap},
    bridge::MixTapWriter,
    host::HostSettingsChange,
    rt::MetronomeConfigChange,
    session::{
        applied_spans,
        decks::{DeckInbox, DeckMsg},
        dispatch::{OwnerPosts, tick_session},
        protocol::{SessionError, SessionSampleRate},
        state::{DeckNode, SessionState, SessionStream, TapSlot, add_graph_node},
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

type Command = HostCommand<TestPools, Queue<TestPools>>;

struct Inbox(kithara_platform::sync::Mutex<kithara_platform::sync::mpsc::Sender<DeckMsg>>);
impl DeckInbox for Inbox {
    fn post(&self, message: DeckMsg) -> Result<(), PlayError> {
        self.0.lock().send(message).map_err(|_| PlayError::Closed)
    }
}

struct TestState {
    owner: HostCore<TestPools, Queue<TestPools>>,
    messages: kithara_platform::sync::mpsc::Receiver<DeckMsg>,
}
impl TestState {
    const DEFAULT_SAMPLE_RATE: u32 = 44_100;
}
impl std::ops::Deref for TestState {
    type Target = SessionState<SessionStream, TestPools>;
    fn deref(&self) -> &Self::Target {
        &self.owner.session
    }
}
impl std::ops::DerefMut for TestState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.owner.session
    }
}

fn test_state(
    start: impl FnMut(&mut FirewheelContext, u32) -> Result<SessionStream, String> + Send + 'static,
) -> TestState {
    let state = crate::session::tests::graph::state_for(
        NonZeroU32::new(TestState::DEFAULT_SAMPLE_RATE).expect("fixture rate"),
        start,
    );
    let (tx, messages) = kithara_platform::sync::mpsc::channel();
    let inbox = Arc::new(Inbox(kithara_platform::sync::Mutex::new(tx)));
    TestState {
        owner: HostCore::new(state, inbox),
        messages,
    }
}

fn ask(state: &mut TestState, command: Command) -> Result<(), PlayError> {
    state.owner.begin_pass();
    for message in state.messages.try_iter() {
        message.run(&mut state.owner);
    }
    state.owner.apply(command)?;
    state.owner.pass();
    state.owner.begin_pass();
    state.owner.pass();
    Ok(())
}

fn render_block(state: &mut TestState, clock: u64) -> Vec<f32> {
    let mut block = vec![0.0; 1024];
    let SessionStream::Offline(stream) = state.stream.as_mut().expect("stream") else {
        panic!("offline fixture")
    };
    stream
        .render(clock, 512, &mut block)
        .expect("offline render");
    block
}

fn render_left(state: &mut TestState, clock: &mut u64, blocks: usize) -> Vec<f32> {
    let mut left = Vec::new();
    for _ in 0..blocks {
        left.extend(render_block(state, *clock).into_iter().step_by(2));
        *clock += 512;
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
) -> Result<SessionStream, String> {
    route_loss(|probe| probe.start_count.fetch_add(1, Ordering::SeqCst));
    if route_loss(|probe| probe.fail_next_start.swap(false, Ordering::SeqCst)) {
        return Err(RouteLossError.to_string());
    }
    OfflineStream::start(
        ctx,
        BackendConfig::builder()
            .sample_rate(NonZeroU32::new(sample_rate).expect("rate"))
            .block_frames(NonZeroU32::new(512).expect("block"))
            .declared_latency(Duration::ZERO)
            .build(),
    )
    .map(SessionStream::Offline)
    .map_err(|error| error.to_string())
}

fn registration(grid_id: BeatGridId) -> Command {
    configured_registration(grid_id, None, None)
}

fn configured_registration(
    grid_id: BeatGridId,
    quantum: Option<NonZeroUsize>,
    budget: Option<NonZeroUsize>,
) -> Command {
    let prep = ResourcePrep::builder()
        .worker(PlayWorker::new(PlayWorkerConfig::builder(pools()).build()))
        .warp(
            kithara_warp::WarpConfig::builder()
                .maybe_render_quantum_frames(quantum)
                .build(),
        )
        .maybe_response_budget_frames(budget)
        .build();
    HostCommand::Register {
        id: grid_id,
        deck: Box::new(Queue::new(QueueConfig::builder().prep(prep).build())),
    }
}

/// Attaches a deck, which the session registers and starts.
fn insert(state: &mut TestState) -> BeatGridId {
    let grid_id = BeatGridId::allocate().expect("fixture player grid id");
    match ask(state, registration(grid_id)) {
        Ok(_) => grid_id,
        Err(err) => panic!("the deck failed to start: {err}"),
    }
}

/// Stops the deck `grid_id` and removes it from the session.
fn remove(state: &mut TestState, grid_id: BeatGridId) {
    assert!(matches!(ask(state, HostCommand::Close(grid_id)), Ok(())));
}

/// Asks the session for `rate` from the next block on.
fn configure_sample_rate(state: &mut TestState, rate: u32) {
    let rate = NonZeroU32::new(rate).expect("a fixture rate is not zero");
    assert!(matches!(
        ask(
            state,
            HostCommand::Configure(HostSettingsChange::SampleRate(rate), When::Next)
        ),
        Ok(())
    ));
}

/// Moves the session's output to a new platform route.
fn change_route_to(state: &mut TestState, _reason: &str) {
    assert!(matches!(ask(state, HostCommand::Restart), Ok(())));
}

fn deck(state: &TestState, index: usize) -> &DeckNode {
    &state.deck_nodes[index]
}

fn deck_count(state: &TestState) -> usize {
    state.deck_nodes.len()
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

fn deck_by_grid(state: &TestState, grid_id: BeatGridId) -> &DeckNode {
    state
        .deck_nodes
        .iter()
        .find(|node| node.id == grid_id)
        .expect("registered deck")
}

#[kithara::test]
fn an_attached_deck_runs_until_it_is_detached() {
    route_loss(RouteLossProbe::reset);
    let mut state = test_state(start_route_loss_stream);
    let host_id = state.root.id();

    let grid_id = insert(&mut state);
    assert!(state.deck_nodes.iter().any(|deck| deck.id == grid_id));
    assert!(state.root_view.holds(grid_id));
    assert!(
        state
            .ctx
            .as_ref()
            .expect("context")
            .contains_node(deck_by_grid(&state, grid_id).node)
    );

    remove(&mut state, grid_id);

    assert_eq!(state.root.id(), host_id);
    assert!(!state.deck_nodes.iter().any(|deck| deck.id == grid_id));
    assert!(!state.root_view.holds(grid_id));
    assert_eq!(deck_count(&state), 0);
}

#[kithara::test]
fn attach_refuses_an_identity_the_session_already_holds() {
    route_loss(RouteLossProbe::reset);
    let mut state = test_state(start_route_loss_stream);
    let grid_id = insert(&mut state);
    let scopes = state.channel_config.values().scopes;

    assert!(matches!(
        ask(&mut state, registration(grid_id)),
        Err(PlayError::Session(SessionError::DeckAttached(refused)))
            if refused == grid_id
    ));
    assert_eq!(state.channel_config.values().scopes, scopes);
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
        ask(&mut state, HostCommand::Close(grid_id)),
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
    state.channel_config = kithara_command::ScopedConfig::builder()
        .scope(kithara_command::ChannelConfig::builder().targets(1).build())
        .build();

    let reply = ask(&mut state, registration(grid_id));

    assert!(matches!(
        reply,
        Err(PlayError::Internal(reason)) if reason == kithara_command::OpenError::Targets {
            targets: kithara_play::DeckMixerConfig::default().slots().get(),
            limit: 1,
        }.to_string()
    ));
    assert_eq!(state.channel_config.values().scope.values().targets, 1);
    assert_eq!(deck_count(&state), 0);
    assert!(!state.deck_nodes.iter().any(|deck| deck.id == grid_id));
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
    let registration =
        configured_registration(grid_id, NonZeroUsize::new(64), NonZeroUsize::new(441));

    assert!(matches!(
        ask(&mut state, registration),
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
    assert!(!state.root_view.holds(grid_id));
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
    let slot_node = deck(&state, 0).node;
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
        state
            .ctx
            .as_ref()
            .expect("context")
            .contains_node(deck(&state, 0).node),
        "route invalidation must keep the player graph logically started"
    );
    assert!(
        state
            .ctx
            .as_ref()
            .is_some_and(|ctx| ctx.contains_node(slot_node)),
        "route invalidation must keep the deck's slot node in the graph"
    );
    assert_eq!(deck(&state, 0).node, slot_node);
    assert!(!state.stream_needs_restart);
}

#[kithara::test]
fn unexpected_stream_stop_restarts_stream_without_dropping_the_deck_slot() {
    route_loss(RouteLossProbe::reset);

    let mut state = test_state(start_route_loss_stream);
    insert(&mut state);
    assert!(state.ctx.is_some());
    assert!(
        state
            .ctx
            .as_ref()
            .expect("context")
            .contains_node(deck(&state, 0).node)
    );
    assert_eq!(
        route_loss(|probe| probe.start_count.load(Ordering::SeqCst)),
        1
    );
    let slot_node = deck(&state, 0).node;
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
        state
            .ctx
            .as_ref()
            .expect("context")
            .contains_node(deck(&state, 0).node),
        "player graph must remain logically started after stream restart"
    );
    assert!(
        state
            .ctx
            .as_ref()
            .is_some_and(|ctx| ctx.contains_node(slot_node)),
        "the deck's slot node must stay in the graph across stream restart"
    );
    assert_eq!(Some(deck(&state, 0).node), Some(slot_node));
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

    let (_postbox, mut mailbox) = mailbox::<Command, PlayError>();

    state.stream = None;
    let mut posts = OwnerPosts::new();
    state.owner.begin_pass();
    posts.drain(&mut state.owner, &mut mailbox);
    posts.pass(&mut state.owner, true);

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
    let mut state = test_state(move |ctx, _| {
        OfflineStream::start(
            ctx,
            BackendConfig::builder()
                .sample_rate(sample_rate)
                .block_frames(NonZeroU32::new(512).expect("block"))
                .declared_latency(Duration::ZERO)
                .build(),
        )
        .map(SessionStream::Offline)
        .map_err(|error| error.to_string())
    });
    crate::session::state::ensure_ctx(&mut state).expect("active browser graph");
    render_block(&mut state, 0);
    assert_eq!(
        state.root_view.grid().state(),
        BeatGridState::Unavailable(BeatGridUnavailable::NoGeometry)
    );

    let tempo = Tempo::new(90.0).expect("valid tempo");
    assert!(
        state
            .owner
            .exec(HostSettingsChange::Tempo(tempo), When::Next, &mut ())
            .is_ok()
    );
    assert!(tick_session(&mut state).is_ok());
    for clock_samples in [512, 1024, 1536] {
        render_block(&mut state, clock_samples);
    }
    let before = state.root_view.grid();
    let (_postbox, mut mailbox) = mailbox::<Command, PlayError>();

    let mut posts = OwnerPosts::new();
    state.owner.begin_pass();
    posts.drain(&mut state.owner, &mut mailbox);
    posts.pass(&mut state.owner, true);

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
    assert!(
        state
            .ctx
            .as_ref()
            .expect("context")
            .contains_node(deck(&state, 0).node)
    );
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
            HostCommand::AttachOutputs {
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
                HostCommand::AttachOutputs {
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
                HostCommand::AttachOutputs {
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
            HostCommand::Configure(
                HostSettingsChange::Metronome(MetronomeConfigChange::Enabled(true)),
                When::Next
            )
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
    let block = render_block(&mut state, 0);
    assert!(
        block.iter().any(|sample| sample.abs() > 0.1),
        "the first block of the next stream clicks session beat 0"
    );
}

/// Sends `change` for the next block.
fn configure_next(state: &mut TestState, change: HostSettingsChange) -> Result<(), PlayError> {
    ask(state, HostCommand::Configure(change, When::Next))
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
            HostCommand::Configure(
                HostSettingsChange::Ducking(SessionDuckingMode::Hard),
                When::Next
            )
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
    match ask(&mut state, HostCommand::Close(second)) {
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
