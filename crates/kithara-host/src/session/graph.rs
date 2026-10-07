use std::num::NonZeroUsize;

use firewheel::{FirewheelContext, node::NodeID};
use kithara_bufpool::HasPool;
use kithara_output::OutputGroup;
use kithara_warp::MapAxis;
use tracing::{debug, warn};

use super::{
    protocol::{AllocatedSlot, PlayerId, SessionError},
    queue::settle_receipts,
    state::{Deck, GraphRegistry, SessionState, TapSlot, Taps, add_graph_node, ensure_ctx},
};
use crate::{
    api::SlotId,
    bridge::slot_channels,
    rt::{PlayerNode, TapNode},
};
pub(super) fn player_index<T, S>(
    state: &SessionState<T, S>,
    player_id: PlayerId,
) -> Result<usize, SessionError> {
    state
        .graph
        .index_by_player(player_id)
        .ok_or(SessionError::PlayerNotFound(player_id))
}
fn graph_state(message: &'static str) -> SessionError {
    SessionError::Graph(message.into())
}

fn deck_at<T, S>(state: &SessionState<T, S>, index: usize) -> Result<&Deck<S>, SessionError> {
    state
        .graph
        .deck(index)
        .ok_or_else(|| graph_state("player index out of range"))
}

fn deck_at_mut<S>(
    graph: &mut GraphRegistry<S>,
    index: usize,
) -> Result<&mut Deck<S>, SessionError> {
    graph
        .deck_mut(index)
        .ok_or_else(|| graph_state("player index out of range"))
}
fn connect_stereo(
    fw_ctx: &mut FirewheelContext,
    from: NodeID,
    to: NodeID,
    label: &'static str,
) -> Result<(), SessionError> {
    fw_ctx
        .connect(from, to, &[(0, 0), (1, 1)], false)
        .map(|_| ())
        .map_err(|err| SessionError::Graph(format!("{label} failed: {err}")))
}

pub(super) mod tap {
    use super::*;
    use crate::api::Tap;

    pub(in crate::session) fn attach<T, S>(
        state: &mut SessionState<T, S>,
        tap: Tap,
        outputs: OutputGroup,
    ) -> Result<(), SessionError> {
        if state.taps.slot(tap).is_some() {
            return Err(SessionError::TapActive);
        }
        let Some(from) = source(state, tap) else {
            *state.taps.slot(tap) = Some(TapSlot::Requested(outputs));
            return Ok(());
        };
        install(state, tap, from, outputs)
    }

    pub(in crate::session) fn detach<T, S>(state: &mut SessionState<T, S>, tap: Tap) {
        let Some(TapSlot::Installed(tap_id)) = state.taps.slot(tap).take() else {
            return;
        };
        let Some(ref mut fw_ctx) = state.ctx else {
            return;
        };
        if let Err(err) = fw_ctx.remove_node(tap_id) {
            warn!(?tap, ?err, "failed to remove session tap node");
        }
        if let Err(err) = fw_ctx.update() {
            warn!(?tap, "graph update after tap detach failed: {err:?}");
        }
    }

    pub(in crate::session) fn install_requested<T, S>(
        state: &mut SessionState<T, S>,
    ) -> Result<(), SessionError> {
        for tap in [Tap::Master, Tap::Output] {
            let Some(from) = source(state, tap) else {
                continue;
            };
            let slot = state.taps.slot(tap);
            if !matches!(slot, Some(TapSlot::Requested(_))) {
                continue;
            }
            let Some(TapSlot::Requested(outputs)) = slot.take() else {
                continue;
            };
            install(state, tap, from, outputs)?;
        }
        Ok(())
    }

    fn source<T, S>(state: &SessionState<T, S>, tap: Tap) -> Option<NodeID> {
        match tap {
            Tap::Master => state.session_limiter_node_id,
            Tap::Output => state.session_metronome_node_id,
        }
    }

    fn install<T, S>(
        state: &mut SessionState<T, S>,
        tap: Tap,
        from: NodeID,
        outputs: OutputGroup,
    ) -> Result<(), SessionError> {
        let fw_ctx = state.ctx.as_mut().ok_or(SessionError::NoContext)?;
        let tap_id = add_graph_node(fw_ctx, TapNode::new(outputs))?;
        if let Err(err) = connect_stereo(fw_ctx, from, tap_id, "connect session output->tap") {
            if let Err(remove_err) = fw_ctx.remove_node(tap_id) {
                warn!(?remove_err, "failed to remove the unconnected tap node");
            }
            return Err(err);
        }
        if let Err(err) = fw_ctx.update() {
            warn!("graph update after tap install failed: {err:?}");
        }
        *state.taps.slot(tap) = Some(TapSlot::Installed(tap_id));
        debug!(?tap, ?tap_id, "[KITHARA-ROUTE] session tap installed");
        Ok(())
    }
}

pub(super) mod lifecycle {
    use super::*;

    pub(in crate::session) fn start_player<T, S>(
        state: &mut SessionState<T, S>,
        player_id: PlayerId,
        render_quantum_frames: Option<NonZeroUsize>,
        response_budget_frames: Option<NonZeroUsize>,
    ) -> Result<AllocatedSlot, SessionError>
    where
        S: HasPool<f32> + Send + Sync + 'static,
    {
        debug!(player_id, "[KITHARA-ROUTE] starting player");
        ensure_ctx(state)?;
        validate_response_geometry(state, render_quantum_frames, response_budget_frames)?;
        let idx = player_index(state, player_id)?;
        if deck_at(state, idx)?.started() {
            return Err(SessionError::AlreadyStarted(player_id));
        }
        let Some(session_output_id) = state.session_output_node_id else {
            return Err(graph_state("session output node is not initialised"));
        };
        let fw_ctx = state.ctx.as_mut().ok_or(SessionError::NoContext)?;
        let player = deck_at_mut(&mut state.graph, idx)?;
        let slot_id = SlotId::new(player.next_slot_id);
        player.next_slot_id += 1;
        let (inputs, control) = slot_channels();
        let player_node =
            PlayerNode::new(inputs, player.pools.clone(), player.mixer).with_session_context();
        let player_node_id = add_graph_node(fw_ctx, player_node)?;
        let player_to_output = "connect player->session_output";
        connect_stereo(fw_ctx, player_node_id, session_output_id, player_to_output)?;
        if let Err(err) = fw_ctx.update() {
            warn!(
                player_id,
                ?slot_id,
                "graph update after player start failed: {err:?}"
            );
        }
        player.slot_node = Some(player_node_id);
        debug!(
            player_id,
            ?slot_id,
            ?player_node_id,
            "[KITHARA-ROUTE] player graph started"
        );
        Ok(AllocatedSlot::new(control, slot_id))
    }

    fn validate_response_geometry<T, S>(
        state: &SessionState<T, S>,
        render_quantum_frames: Option<NonZeroUsize>,
        response_budget_frames: Option<NonZeroUsize>,
    ) -> Result<(), SessionError> {
        let Some(render_quantum_frames) = render_quantum_frames else {
            return Ok(());
        };
        let info = state
            .ctx
            .as_ref()
            .and_then(FirewheelContext::stream_info)
            .ok_or(SessionError::NoContext)?;
        kithara_play::StreamShape::new(info.max_block_frames, info.sample_rate)
            .playback_buffers(render_quantum_frames, response_budget_frames)?;
        Ok(())
    }
    pub(in crate::session) fn stop_player<T, S>(
        state: &mut SessionState<T, S>,
        player_id: PlayerId,
    ) -> Result<(), SessionError> {
        debug!(player_id, "[KITHARA-ROUTE] stopping player");
        let idx = player_index(state, player_id)?;
        stop_player_idx(state, idx)
    }
    fn stop_player_idx<T, S>(
        state: &mut SessionState<T, S>,
        idx: usize,
    ) -> Result<(), SessionError> {
        {
            let (ctx, graph) = (&mut state.ctx, &mut state.graph);
            let player = deck_at_mut(graph, idx)?;
            let player_id = player.player_id;
            let Some(slot_node) = player.slot_node.take() else {
                return Err(SessionError::NotRunning(player_id));
            };
            if let Some(fw_ctx) = ctx {
                if let Err(err) = fw_ctx.remove_node(slot_node) {
                    warn!(player_id, ?err, "failed to remove the deck's slot node");
                }
                if let Err(err) = fw_ctx.update() {
                    warn!(player_id, "graph update after player stop failed: {err:?}");
                }
            }
        }
        shutdown_if_idle(state)?;
        debug!("[KITHARA-ROUTE] player stopped");
        Ok(())
    }
    /// Release the output device once no deck is left to feed it: the Host
    /// keeps the output engaged only while it holds a deck, and the next
    /// `start_player` builds a fresh context.
    ///
    /// A session that set [`SessionState::retains_output`] is the exception:
    /// its device cannot be rebuilt, so this call does nothing.
    ///
    /// Reserves a successor before stopping, since backends may defer processor
    /// drop after `stop_stream` and teardown must not depend on the RT
    /// `stream_stopped` callback reaching this handle.
    pub(in crate::session) fn shutdown_if_idle<T, S>(
        state: &mut SessionState<T, S>,
    ) -> Result<(), SessionError> {
        if state.retains_output {
            return Ok(());
        }
        let idle = state.graph.decks().all(|deck| !deck.started());
        if idle {
            debug!("[KITHARA-ROUTE] shutting down idle session stream");
            if state.ctx.is_none() {
                return Err(SessionError::NoContext);
            }
            let observed_session_grid = state
                .transport_control
                .as_mut()
                .ok_or_else(|| {
                    graph_state("session transport control is missing during idle shutdown")
                })?
                .observation()
                .session_grid();
            let mut session_grid_generation = match state.reserved_session_grid {
                Some(reserved) => reserved
                    .promote(observed_session_grid)
                    .map_err(|error| graph_state(error.message()))?,
                None => observed_session_grid,
            };
            session_grid_generation
                .advance_restart()
                .map_err(|error| graph_state(error.message()))?;
            let session_stamp = session_grid_generation
                .stamp()
                .map_err(|error| graph_state(error.message()))?;
            let MapAxis::Session(axis) = state.root.grid().axis() else {
                return Err(graph_state(
                    "session host published a non-session grid during idle shutdown",
                ));
            };
            let sample_rate = axis.sample_rate();
            state.root.publish_unavailable(
                session_stamp,
                sample_rate,
                session_grid_generation.epoch(),
            );
            state.publish_root();
            state.reserved_session_grid = Some(session_grid_generation);
            state
                .ctx
                .as_mut()
                .ok_or(SessionError::NoContext)?
                .request_deactivate();
            state.stream = None;
            state.ctx = None;
            settle_receipts(state);
            state.settings.abandon();
            state.publish_root();
            state.transport_control = None;
            state.taps = Taps::default();
            state.session_output_node_id = None;
            state.session_limiter_node_id = None;
            state.session_metronome_node_id = None;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, num::NonZeroU32};

    use audioadapter_buffers::direct::InterleavedSlice;
    use firewheel::{
        ActivateInfo, backend::BackendProcessInfo, node::StreamStatus,
        processor::FirewheelProcessor,
    };
    use kithara_command::When;
    use kithara_events::EventBus;
    use kithara_platform::time::Duration;
    use kithara_signal::{SessionEpoch, SessionFrame};
    use kithara_test_utils::{
        bufpool::{TestPools, pools},
        kithara,
    };
    use kithara_warp::{
        Beat, BeatGridId, BeatGridQuery, BeatGridRevision, BeatGridState, BeatGridUnavailable,
        MapAxis, MapPoint, MapPosition, SessionAxis,
    };

    use super::*;
    use crate::{
        api::{SessionTransportSnapshot, Tempo},
        consts,
        host::{HostSettingsChange, HostSettingsExec},
        session::{
            dispatch::{invalidate_audio_route, run_cmd, run_host_cmd},
            protocol::{Cmd, DeckRegistration, HostCmd, HostReply, Reply},
            tests::graph::{committed_transport, state as test_state},
        },
    };

    /// The process-wide output device, held by whichever stream owns it.
    #[derive(Default)]
    struct AudioDevice {
        processor: Option<FirewheelProcessor>,
        retired_processors: Vec<FirewheelProcessor>,
        defer_processor_drop: bool,
        next_stream: u64,
        owner: u64,
    }

    thread_local! {
        static DEVICE: RefCell<AudioDevice> = RefCell::new(AudioDevice::default());
    }

    fn device<R>(f: impl FnOnce(&mut AudioDevice) -> R) -> R {
        DEVICE.with(|cell| f(&mut cell.borrow_mut()))
    }

    /// A fixture stream. It owns nothing but its identity: the processor lives
    /// in the thread-local device, and dropping the stream is what retires it.
    struct TestStream {
        stream: u64,
    }

    type TestState = SessionState<TestStream, TestPools>;

    impl Drop for TestStream {
        fn drop(&mut self) {
            device(|dev| {
                if dev.owner == self.stream {
                    let processor = dev.processor.take();
                    if dev.defer_processor_drop {
                        dev.retired_processors.extend(processor);
                    }
                }
            });
        }
    }

    fn start_test_stream(
        ctx: &mut FirewheelContext,
        sample_rate: u32,
    ) -> Result<TestStream, String> {
        let stream = device(|dev| {
            dev.next_stream += 1;
            dev.next_stream
        });
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
        device(|dev| {
            dev.owner = stream;
            dev.processor = Some(processor);
        });
        Ok(TestStream { stream })
    }

    /// `false` means no stream owns the device, which is what silence looks like.
    fn deliver_one_block() -> bool {
        device(|dev| {
            let Some(processor) = dev.processor.as_mut() else {
                return false;
            };
            let mut output = [0.0_f32; consts::GRAPH_BLOCK_FRAMES * 2];
            let input = InterleavedSlice::new(&[] as &[f32], 0, 0)
                .expect("invariant: an empty input adapter is well formed");
            let mut output = InterleavedSlice::new_mut(&mut output, 2, consts::GRAPH_BLOCK_FRAMES)
                .expect("invariant: the fixture output block is stereo");
            processor.process(
                &input,
                &mut output,
                BackendProcessInfo {
                    frames: consts::GRAPH_BLOCK_FRAMES,
                    // Firewheel stamps a block with its own clock type, so the
                    // platform clock cannot be handed over here.
                    process_timestamp: Some(bevy_platform::time::Instant::now()),
                    duration_since_stream_start: Duration::ZERO,
                    input_stream_status: StreamStatus::empty(),
                    output_stream_status: StreamStatus::empty(),
                    dropped_frames: 0,
                    process_to_playback_delay: None,
                },
            );
            true
        })
    }

    fn processed_frames(state: &TestState) -> i64 {
        state
            .ctx
            .as_ref()
            .map_or(-1, |fw_ctx| fw_ctx.audio_clock().samples.0)
    }

    /// Attaches a deck, which the session registers and starts.
    fn insert(state: &mut TestState) -> BeatGridId {
        let grid_id = BeatGridId::allocate().expect("fixture grid id");
        let mut registration = DeckRegistration::new(
            grid_id,
            EventBus::default(),
            pools(),
            kithara_play::DeckMixerConfig::default(),
        );
        registration.response_budget_frames = NonZeroUsize::new(448);
        match run_host_cmd(state, HostCmd::Attach { registration }) {
            HostReply::Attached(_) => grid_id,
            HostReply::Err(err) => panic!("the deck failed to start: {err}"),
            _ => panic!("attach returned an unexpected reply"),
        }
    }

    /// Stops the deck `grid_id` and removes it from the session.
    fn remove(state: &mut TestState, grid_id: BeatGridId) {
        match run_host_cmd(state, HostCmd::Detach { grid_id }) {
            HostReply::Ok => {}
            HostReply::Err(err) => panic!("the deck failed to leave: {err}"),
            _ => panic!("detach returned an unexpected reply"),
        }
    }

    fn slot_node(state: &TestState, grid_id: BeatGridId) -> NodeID {
        state
            .graph
            .decks()
            .find(|deck| deck.grid_id == grid_id)
            .and_then(|deck| deck.slot_node)
            .expect("a started deck has its slot node")
    }

    fn render_and_read_session_grid(state: &mut TestState) -> SessionTransportSnapshot {
        assert!(deliver_one_block(), "the transport must render a block");
        committed_transport(state).expect("the rendered block committed the transport")
    }

    #[kithara::test]
    fn a_session_tick_publishes_the_session_grid_the_graph_committed() {
        device(|dev| *dev = AudioDevice::default());
        let mut state = test_state(start_test_stream);
        insert(&mut state);
        assert!(deliver_one_block(), "the transport must render a block");

        assert!(matches!(run_cmd(&mut state, Cmd::Tick), Reply::Ok));

        let committed = state
            .transport_control
            .as_mut()
            .expect("a running stream keeps transport control")
            .observation()
            .snapshot()
            .expect("the rendered block committed the tempo")
            .session_grid();
        assert_eq!(
            state.root_view.grid(),
            committed,
            "with no synchronization command, the session tick publishes the committed grid"
        );
    }

    /// The Host keeps the output engaged only while it holds a deck: handing
    /// back the last one releases the device, so the platform's audio session
    /// can be deactivated.
    #[kithara::test]
    fn removing_the_last_deck_releases_the_output() {
        device(|dev| *dev = AudioDevice::default());
        let mut state = test_state(start_test_stream);
        let deck = insert(&mut state);

        remove(&mut state, deck);

        assert!(state.ctx.is_none());
    }

    /// The browser hands its output over once, on a user gesture, and a closed
    /// `AudioContext` can never be resumed: releasing it on idle leaves every
    /// later context suspended, so the player reports playback over silence.
    /// The exception belongs to the session that declared it, not to the
    /// target it happens to be compiled for — a mock backend on the same
    /// target still releases its device above.
    #[kithara::test]
    fn a_session_that_retains_its_output_keeps_it_when_the_last_deck_leaves() {
        device(|dev| *dev = AudioDevice::default());
        let mut state = test_state(start_test_stream);
        state.retains_output = true;
        let deck = insert(&mut state);

        remove(&mut state, deck);

        assert!(
            state.ctx.is_some(),
            "a session whose device cannot be rebuilt must hold it while idle"
        );
    }

    /// A removed deck takes its slot node out of the graph the other decks
    /// keep rendering.
    #[kithara::test]
    fn a_removed_deck_takes_its_slot_node_out_of_the_graph() {
        device(|dev| *dev = AudioDevice::default());
        let mut state = test_state(start_test_stream);
        let leaving = insert(&mut state);
        let staying = insert(&mut state);
        let (left, kept) = (slot_node(&state, leaving), slot_node(&state, staying));

        remove(&mut state, leaving);

        let ctx = state
            .ctx
            .as_ref()
            .expect("the deck still playing keeps the output");
        assert!(
            !ctx.contains_node(left),
            "a removed deck's slot node leaves the graph"
        );
        assert!(
            ctx.contains_node(kept),
            "the deck still playing keeps its slot node"
        );
    }

    #[kithara::test]
    fn a_second_player_started_after_the_last_one_left_gets_a_processed_stream() {
        device(|dev| *dev = AudioDevice::default());
        let mut state = test_state(start_test_stream);

        let first = insert(&mut state);
        assert!(
            deliver_one_block(),
            "the first player's stream must own the output device"
        );

        remove(&mut state, first);

        assert!(
            state.ctx.is_none(),
            "the session must release the output device once no player feeds it"
        );

        insert(&mut state);

        let before = processed_frames(&state);
        assert!(
            deliver_one_block(),
            "the second player's stream must own the output device"
        );
        assert!(
            processed_frames(&state) > before,
            "the second player's stream delivered no processed callback"
        );
    }

    #[kithara::test]
    fn idle_context_recreation_advances_generation_before_deferred_processor_drop() {
        device(|dev| {
            *dev = AudioDevice::default();
            dev.defer_processor_drop = true;
        });
        let mut state = test_state(start_test_stream);
        let initial = state.root.grid().clone();
        assert_eq!(initial.revision(), BeatGridRevision::first());
        assert_eq!(
            initial.state(),
            BeatGridState::Unavailable(BeatGridUnavailable::NoGeometry)
        );
        assert_eq!(
            initial.axis(),
            MapAxis::Session(SessionAxis::new(
                NonZeroU32::new(TestState::DEFAULT_SAMPLE_RATE)
                    .expect("the fixture sample rate is non-zero"),
                SessionEpoch::new(0),
            ))
        );
        assert_eq!(
            state
                .reserved_session_grid
                .expect("the session seeds its session-grid generation")
                .stamp()
                .expect("the initial session-grid revision is committed"),
            initial.stamp()
        );
        let first_player = insert(&mut state);
        let before = render_and_read_session_grid(&mut state);
        let first_live = state.root.grid().clone();
        assert_eq!(first_live, before.session_grid());
        assert_eq!(
            first_live.revision(),
            initial
                .revision()
                .checked_next()
                .expect("the fixture grid revision can advance")
        );
        let old_beat = MapPoint::new(
            before.session_grid_stamp(),
            Beat::new(1.0).expect("invariant: fixture beat is finite"),
        );

        invalidate_audio_route(&mut state, "deferred route before idle teardown")
            .expect("the route restarts");
        let route_boundary = state.root.grid().clone();
        assert_eq!(
            state
                .reserved_session_grid
                .expect("the deferred route owns a reserved generation")
                .stamp()
                .expect("the deferred route reservation has a revision"),
            route_boundary.stamp()
        );
        assert!(state.stream_needs_restart);

        remove(&mut state, first_player);
        let unavailable = state.root.grid().clone();
        assert_eq!(
            unavailable.revision(),
            first_live
                .revision()
                .checked_next()
                .and_then(BeatGridRevision::checked_next)
                .expect("the fixture grid revision can advance twice")
        );
        assert_eq!(
            unavailable.state(),
            BeatGridState::Unavailable(BeatGridUnavailable::NoGeometry)
        );
        assert_eq!(
            unavailable.axis(),
            MapAxis::Session(SessionAxis::new(
                NonZeroU32::new(TestState::DEFAULT_SAMPLE_RATE)
                    .expect("the fixture sample rate is non-zero"),
                SessionEpoch::new(2),
            ))
        );
        assert_eq!(
            state
                .reserved_session_grid
                .expect("idle teardown returns session-grid generation")
                .stamp()
                .expect("the restart boundary has a reserved revision"),
            unavailable.stamp()
        );
        assert!(
            state.ctx.is_none(),
            "idle teardown must destroy the context"
        );
        device(|dev| {
            assert_eq!(
                dev.retired_processors.len(),
                1,
                "old processor must still be alive while the new context starts"
            );
        });

        insert(&mut state);
        let after = render_and_read_session_grid(&mut state);
        let second_live = state.root.grid().clone();
        assert_eq!(second_live, after.session_grid());
        assert_eq!(
            second_live.revision(),
            unavailable
                .revision()
                .checked_next()
                .expect("the fixture grid revision can advance")
        );

        assert_eq!(
            before.session_grid_stamp().grid_id(),
            after.session_grid_stamp().grid_id(),
            "one session keeps one session-grid identity"
        );
        assert!(after.session_epoch() > before.session_epoch());
        assert!(after.session_grid_stamp().revision() > before.session_grid_stamp().revision());
        assert!(matches!(
            after.session_grid().position_at(old_beat),
            BeatGridQuery::Stale { expected, given }
                if expected == after.session_grid_stamp()
                    && given == before.session_grid_stamp()
        ));
        device(|dev| {
            assert_eq!(dev.retired_processors.len(), 1);
            dev.retired_processors.clear();
            dev.defer_processor_drop = false;
        });
    }

    #[kithara::test]
    fn deferred_route_restart_converges_before_unrendered_idle_shutdown() {
        device(|dev| {
            *dev = AudioDevice::default();
            dev.defer_processor_drop = true;
        });
        let mut state = test_state(start_test_stream);
        let player = insert(&mut state);
        let live = render_and_read_session_grid(&mut state);

        invalidate_audio_route(&mut state, "test route restart").expect("the route restarts");
        let reserved = state.root.grid().clone();
        assert!(reserved.revision() > live.session_grid_stamp().revision());
        assert_eq!(
            state
                .reserved_session_grid
                .expect("the delayed processor keeps an exact route reservation")
                .stamp()
                .expect("the route reservation has a revision"),
            reserved.stamp()
        );
        assert!(state.stream_needs_restart);
        let rate = state.root_view.sample_rate();
        assert_eq!(
            rate.measured, None,
            "a pending route restart publishes no measured stream"
        );
        assert_eq!(rate.requested, 44_100);
        assert_eq!(
            committed_transport(&mut state),
            None,
            "a route restart holding the session grid has nothing committed to read"
        );
        assert_eq!(
            state.root.grid().clone(),
            reserved,
            "a stale transport observation must not replace the route reservation"
        );
        let tempo = Tempo::new(121.0).expect("invariant: fixture tempo is valid");
        assert!(
            state
                .exec(HostSettingsChange::Tempo(tempo), When::Next, &mut ())
                .is_ok(),
            "a change for the next block waits in the queue across a route restart"
        );
        assert_eq!(
            state.root.grid().clone(),
            reserved,
            "a queued change must not touch an unfinished route boundary"
        );
        device(|dev| {
            assert_eq!(dev.retired_processors.len(), 1);
            dev.retired_processors.clear();
            dev.defer_processor_drop = false;
        });
        assert!(matches!(run_cmd(&mut state, Cmd::Tick), Reply::Ok));
        assert!(!state.stream_needs_restart);
        assert!(state.reserved_session_grid.is_none());
        let converged = state
            .transport_control
            .as_mut()
            .expect("the restarted stream keeps transport control")
            .observation()
            .session_grid()
            .stamp()
            .expect("the restarted transport has a grid revision");
        assert_eq!(converged, reserved.stamp());
        let restart_frame = SessionFrame::new(
            state
                .ctx
                .as_ref()
                .expect("the restarted stream keeps its context")
                .audio_clock()
                .samples
                .0,
        );

        assert!(
            deliver_one_block(),
            "the restarted processor must render its preserved transport"
        );
        let restarted = committed_transport(&mut state)
            .expect("the restarted transport committed its first block");
        let published = state.root.grid().clone();
        assert_eq!(published.state(), BeatGridState::Live);
        let MapAxis::Session(reserved_axis) = reserved.axis() else {
            panic!("the route reservation uses the session axis")
        };
        let MapAxis::Session(published_axis) = published.axis() else {
            panic!("the restarted grid uses the session axis")
        };
        assert_eq!(published_axis.epoch(), reserved_axis.epoch());
        assert!(published.revision() > reserved.revision());
        assert_eq!(published, restarted.session_grid());
        assert_eq!(
            restarted
                .anchor()
                .frame_at(live.position())
                .expect("the preserved beat is representable on the restarted axis"),
            restart_frame
        );
        let old_position = MapPoint::new(
            live.session_grid_stamp(),
            MapPosition::Session(SessionFrame::new(0)),
        );
        assert!(matches!(
            published.beat_at(old_position),
            BeatGridQuery::Stale { .. }
        ));

        remove(&mut state, player);

        let unavailable = state.root.grid().clone();
        assert!(unavailable.revision() > reserved.revision());
        assert_eq!(
            unavailable.state(),
            BeatGridState::Unavailable(BeatGridUnavailable::NoGeometry)
        );
        assert!(state.ctx.is_none());
    }
}
