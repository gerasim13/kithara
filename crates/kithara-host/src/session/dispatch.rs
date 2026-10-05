use std::num::NonZeroU32;

use firewheel::{FirewheelContext, error::UpdateError};
use kithara_bufpool::HasPool;
use kithara_command::When;
use kithara_config::ConfigOwner;
#[cfg(any(target_arch = "wasm32", test))]
use kithara_platform::sync::mpsc;
use kithara_play::{PlayError, StreamShape};
use kithara_sync::{
    SyncCapability, SyncError, SyncGroup, SyncOperation, SyncReceipt, SyncRejected,
    SyncStatusSnapshot, TopologyOperation,
};
use tracing::{debug, trace, warn};

#[cfg(any(target_arch = "wasm32", test))]
use super::protocol::HostCmdMsg;
use super::{
    graph::{controls, lifecycle, player_index, slots, tap},
    protocol::{
        Cmd, HostCmd, HostReply, PlayerId, PlayerLevel, Reply, SessionError, SessionSampleRate,
        SyncCmd,
    },
    queue::settle_receipts,
    state::{SessionState, register_player},
    transport,
    transport::RouteRestartStatus,
};
use crate::{
    PlayerMember,
    api::HostLevel,
    host::{HostSettingsChange, HostSettingsExec},
};

/// Runs one Host command after settling the transport's receipts, so the
/// queue's credits come back and the settings catch up even while no tick
/// runs.
pub(crate) fn run_host_cmd<T, S>(state: &mut SessionState<T, S>, cmd: HostCmd<S>) -> HostReply
where
    S: HasPool<f32> + Send + Sync + 'static,
{
    settle_receipts(state);
    match cmd {
        HostCmd::Play(cmd) => HostReply::Play(run_cmd(state, cmd)),
        HostCmd::Sync(cmd) => run_sync_cmd(state, cmd),
        HostCmd::ApplyMix { levels } => {
            apply_mix(state, &levels).map_or_else(HostReply::Err, |()| HostReply::Ok)
        }
        HostCmd::Configure { change, at } => state
            .exec(change, at, &mut ())
            .map_or_else(HostReply::Err, |()| HostReply::Ok),
        HostCmd::AttachOutputs {
            tap: target,
            outputs,
        } => tap::attach(state, target, outputs)
            .map_or_else(|error| HostReply::Err(error.into()), |()| HostReply::Ok),
        HostCmd::DetachOutputs { tap: target } => {
            tap::detach(state, target);
            HostReply::Ok
        }
        HostCmd::Shutdown => HostReply::Ok,
    }
}

fn run_sync_cmd<T, S>(state: &mut SessionState<T, S>, cmd: SyncCmd) -> HostReply {
    let operation = match cmd {
        SyncCmd::Transact(operation) => match transport::observe_commits(state) {
            Ok(()) => operation,
            Err(error) => return HostReply::Admission(Err(SyncRejected::new(error, operation))),
        },
        SyncCmd::TransactCurrent(operations) => {
            let topology =
                match transport::observe_commits(state).and_then(|()| state.root.topology()) {
                    Ok(topology) => topology,
                    Err(error) => return HostReply::Err(SessionError::from(error).into()),
                };
            SyncOperation::Topology {
                operations,
                base: topology.stamp(),
            }
        }
        SyncCmd::Acknowledge(receipt) => {
            return HostReply::Acknowledged(acknowledge_root(state, receipt));
        }
    };
    let result = transact_root(state, operation);
    if result.is_ok() {
        state.publish_root();
    }
    HostReply::Admission(result)
}

/// Records one executor receipt on the root group and publishes the state it
/// leaves; a refused receipt changes nothing and publishes nothing.
fn acknowledge_root<T, S>(
    state: &mut SessionState<T, S>,
    receipt: SyncReceipt,
) -> Result<SyncStatusSnapshot, SyncError> {
    transport::observe_commits(state)?;
    let result = state.root.acknowledge(receipt);
    if result.is_ok() {
        state.publish_root();
    }
    result
}

fn transact_root<T, S>(
    state: &mut SessionState<T, S>,
    operation: SyncOperation<PlayerMember>,
) -> Result<kithara_sync::SyncAdmission, SyncRejected<PlayerMember>> {
    if topology_conflicts_with_graph(state, &operation) {
        return Err(SyncRejected::new(
            SyncError::CapabilityUnavailable {
                capability: SyncCapability::Topology,
            },
            operation,
        ));
    }
    state.root.transact(operation)
}

fn topology_conflicts_with_graph<T, S>(
    state: &SessionState<T, S>,
    operation: &SyncOperation<PlayerMember>,
) -> bool {
    let SyncOperation::Topology { operations, .. } = operation else {
        return false;
    };
    operations.iter().any(|operation| match operation {
        TopologyOperation::Attach { member } => state.graph.index_by_grid(member.id()).is_some(),
        TopologyOperation::Detach { member } => state.graph.index_by_grid(*member).is_some(),
        TopologyOperation::Replace {
            member,
            replacement,
        } => {
            state.graph.index_by_grid(*member).is_some()
                || state.graph.index_by_grid(replacement.id()).is_some()
        }
    })
}

pub(crate) fn run_cmd<T, S>(state: &mut SessionState<T, S>, cmd: Cmd<S>) -> Reply
where
    S: HasPool<f32> + Send + Sync + 'static,
{
    match cmd {
        Cmd::RegisterPlayer {
            grid_id,
            bus,
            eq_layout,
            gate_smoothing,
            pools,
        } => match register_player(state, grid_id, bus, eq_layout, pools, gate_smoothing) {
            Ok(player_id) => Reply::PlayerRegistered(player_id),
            Err(error) => Reply::Err(error),
        },
        Cmd::UnregisterPlayer { player_id } => match unregister_player(state, player_id) {
            Ok(()) => Reply::Ok,
            Err(err) => Reply::Err(err),
        },
        Cmd::StartPlayer {
            master_volume,
            player_id,
            render_quantum_frames,
            response_budget_frames,
        } => match lifecycle::start_player(
            state,
            player_id,
            master_volume,
            render_quantum_frames,
            response_budget_frames,
        ) {
            Ok(()) => Reply::Ok,
            Err(err) => Reply::Err(err),
        },
        Cmd::StopPlayer { player_id } => match lifecycle::stop_player(state, player_id) {
            Ok(()) => Reply::Ok,
            Err(err) => Reply::Err(err),
        },
        Cmd::AllocateSlot { player_id } => {
            slots::allocate_slot(state, player_id).unwrap_or_else(Reply::Err)
        }
        Cmd::ReleaseSlot { player_id, slot } => match slots::release_slot(state, player_id, slot) {
            Ok(()) => Reply::Ok,
            Err(err) => Reply::Err(err),
        },
        Cmd::SetPlayerMasterVolumes { levels } => {
            match controls::set_player_master_volumes(state, &levels) {
                Ok(()) => Reply::Ok,
                Err(err) => Reply::Err(err),
            }
        }
        Cmd::SetPlayerSlotVolume {
            player_id,
            slot,
            volume,
        } => match controls::set_player_slot_volume(state, player_id, slot, volume) {
            Ok(()) => Reply::Ok,
            Err(err) => Reply::Err(err),
        },
        Cmd::SetPlayerEqGain {
            band,
            gain_db,
            player_id,
        } => match controls::set_player_eq_gain(state, player_id, band, gain_db) {
            Ok(()) => Reply::Ok,
            Err(err) => Reply::Err(err),
        },
        Cmd::SetPlayerEqLayout {
            eq_layout,
            player_id,
        } => match controls::set_player_eq_layout(state, player_id, eq_layout) {
            Ok(()) => Reply::Ok,
            Err(err) => Reply::Err(err),
        },
        Cmd::SetSessionDucking { mode } => {
            match state.exec(HostSettingsChange::Ducking(mode), When::Next, &mut ()) {
                Ok(()) => Reply::Ok,
                Err(PlayError::Session(error)) => Reply::Err(error),
                Err(error) => Reply::Err(SessionError::Graph(error.to_string())),
            }
        }
        Cmd::QuerySessionTransport => match transport::snapshot(state) {
            Ok(snapshot) => Reply::SessionTransport(snapshot),
            Err(err) => Reply::Err(err),
        },
        Cmd::InvalidateAudioRoute { reason } => match invalidate_audio_route(state, &reason) {
            Ok(()) => Reply::Ok,
            Err(err) => Reply::Err(err),
        },
        Cmd::QuerySampleRate => {
            trace_stream_info(state, "query-sample-rate");
            Reply::SampleRate(sample_rate(state))
        }
        Cmd::QueryStreamShape => Reply::StreamShape(stream_shape(state)),
        Cmd::Tick => tick_session(state),
        Cmd::AcknowledgeSync { receipt } => match acknowledge_root(state, receipt) {
            Ok(_) => Reply::Ok,
            Err(error) => Reply::Err(SessionError::Sync(error)),
        },
    }
}

/// The shape of the stream the session is actually running on, if it is
/// running on one. Firewheel keeps a deactivated context's stream description
/// until the processor comes back, so a session awaiting a restart would
/// otherwise keep reporting the route it has already disowned as measured.
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

pub(super) fn tick_session<T, S>(state: &mut SessionState<T, S>) -> Reply {
    if state.stream_needs_restart {
        match restart_stream(state) {
            Ok(()) => {}
            Err(err) => {
                warn!(?err, "[KITHARA-ROUTE] deferred stream restart failed");
                return Reply::Err(SessionError::RestartFailed {
                    reason: "deferred stream restart".into(),
                    r#source: err.to_string(),
                });
            }
        }
        if state.stream_needs_restart {
            return Reply::Ok;
        }
    }

    let update = state.ctx.as_mut().map(FirewheelContext::update);
    if let Some(Err(err)) = update {
        return handle_update_error(state, &err);
    }
    if stream_died(state) {
        return restart_dead_stream(state);
    }
    let observed = transport::observe_commits(state);
    settle_receipts(state);
    match observed {
        Ok(()) => Reply::Ok,
        Err(error) => Reply::Err(error.into()),
    }
}

#[cfg(any(target_arch = "wasm32", test))]
pub(super) fn drain_host_channel<T, S>(
    state: &mut SessionState<T, S>,
    rx: &mpsc::Receiver<HostCmdMsg<S>>,
    mut observe: impl FnMut(&HostReply),
) where
    S: HasPool<f32> + Send + Sync + 'static,
{
    for msg in rx.try_iter() {
        let reply = run_host_cmd(state, msg.cmd);
        observe(&reply);
        msg.reply_tx.send(reply).ok();
    }

    if let Reply::Err(err) = tick_session(state) {
        warn!(?err, "session tick in host drain failed");
    }
}

fn unregister_player<T, S>(
    state: &mut SessionState<T, S>,
    player_id: PlayerId,
) -> Result<(), SessionError> {
    debug!(player_id, "[KITHARA-ROUTE] unregistering player");
    let idx = player_index(state, player_id)?;
    let started = state
        .graph
        .deck(idx)
        .ok_or_else(|| SessionError::Graph("registered deck is missing".to_owned()))?
        .started;
    if started {
        lifecycle::stop_player(state, player_id)?;
    } else if state.ctx.is_some() {
        lifecycle::shutdown_if_idle(state)?;
    }
    state
        .graph
        .remove(idx)
        .ok_or_else(|| SessionError::Graph("registered deck is missing".to_owned()))?;
    debug!(
        player_id,
        players = state.graph.len(),
        "[KITHARA-ROUTE] player unregistered"
    );
    Ok(())
}

fn apply_mix<T, S>(state: &mut SessionState<T, S>, levels: &[HostLevel]) -> Result<(), PlayError> {
    let mut projected: Vec<PlayerLevel> = Vec::with_capacity(levels.len());
    for (index, &HostLevel { grid_id, level }) in levels.iter().enumerate() {
        if !level.is_finite() || !(0.0..=1.0).contains(&level) {
            return Err(PlayError::MixLevel { level });
        }
        if levels[..index]
            .iter()
            .any(|candidate| candidate.grid_id == grid_id)
        {
            return Err(PlayError::MixDuplicatePlayer);
        }
        if state.root.with_group(grid_id, |_| ()).is_none() {
            return Err(PlayError::MixForeignSession);
        }
        if let Some(deck_index) = state.graph.index_by_grid(grid_id) {
            let player_id = state
                .graph
                .deck(deck_index)
                .ok_or_else(|| PlayError::Internal("projected player is missing".into()))?
                .player_id;
            projected.push(PlayerLevel::new(player_id, level));
        }
    }

    controls::set_player_master_volumes(state, &projected)?;
    for &HostLevel { grid_id, level } in levels {
        let updated = state.root.with_group(grid_id, |member| {
            member.commit_host_level(level);
        });
        if updated.is_none() {
            return Err(PlayError::MixForeignSession);
        }
    }
    Ok(())
}

pub(super) fn handle_update_error<T, S>(
    _state: &mut SessionState<T, S>,
    err: &UpdateError,
) -> Reply {
    warn!(?err, "[KITHARA-ROUTE] firewheel update failed");
    Reply::Err(SessionError::Graph(format!("{err:?}")))
}

/// A context that went inactive under a session that believes its stream is
/// running lost that stream: Firewheel hands the processor back when it stops,
/// and since 0.14 that is the only place the death shows up — it is no longer
/// reported as an update error.
pub(super) fn stream_died<T, S>(state: &SessionState<T, S>) -> bool {
    !state.stream_needs_restart && state.ctx.as_ref().is_some_and(|ctx| !ctx.is_active())
}

fn restart_dead_stream<T, S>(state: &mut SessionState<T, S>) -> Reply {
    state.stream_needs_restart = true;
    state.publish_root();
    warn!("session stream stopped unexpectedly; restarting audio stream");
    trace!(
        sample_rate = state.settings.config().sample_rate().get(),
        "[KITHARA-ROUTE] firewheel context went inactive under a live stream"
    );
    match restart_stream(state) {
        Ok(()) => Reply::Ok,
        Err(restart_err) => Reply::Err(SessionError::RestartFailed {
            reason: "audio stream stopped".to_owned(),
            r#source: restart_err.to_string(),
        }),
    }
}

pub(super) fn invalidate_audio_route<T, S>(
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
    use kithara_command::When;
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
    use kithara_play::{DEFAULT_GATE_SMOOTHING, Tempo};
    use kithara_sync::SyncGroupSnapshot;
    use kithara_test_utils::{
        bufpool::{TestPools, pools},
        kithara,
    };
    use kithara_warp::{BeatGrid, BeatGridSnapshot, BeatGridState, BeatGridUnavailable, MapAxis};
    use ringbuf::{HeapRb, traits::Split};

    use super::*;
    use crate::{
        api::{SessionDuckingMode, Tap},
        bridge::MixTapWriter,
        host::HostSettingsChange,
        rt::MetronomeConfigChange,
        session::{
            applied_spans,
            graph::master_gain,
            protocol::{Cmd, Reply, SessionError},
            state::{Deck, SessionState, TapSlot, add_graph_node},
            tests::{
                graph::{attach_player, state as test_state},
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

    fn register_command(grid_id: kithara_warp::BeatGridId) -> Cmd<TestPools> {
        Cmd::RegisterPlayer {
            grid_id,
            bus: EventBus::default(),
            eq_layout: Vec::new(),
            gate_smoothing: DEFAULT_GATE_SMOOTHING,
            pools: pools(),
        }
    }

    fn register_player(state: &mut TestState) -> u64 {
        let grid_id = attach_player(state);
        match run_cmd(state, register_command(grid_id)) {
            Reply::PlayerRegistered(registered) => registered.id,
            Reply::Err(err) => panic!("player registration failed: {err}"),
            _ => panic!("player registration returned unexpected reply"),
        }
    }

    /// Asks the session for `rate` from the next block on.
    fn configure_sample_rate(state: &mut TestState, rate: u32) {
        let rate = NonZeroU32::new(rate).expect("a fixture rate is not zero");
        assert!(matches!(
            run_host_cmd(
                state,
                HostCmd::Configure {
                    change: HostSettingsChange::SampleRate(rate),
                    at: When::Next,
                },
            ),
            HostReply::Ok
        ));
    }

    fn start_command(player_id: u64) -> Cmd<TestPools> {
        Cmd::StartPlayer {
            player_id,
            master_volume: 1.0,
            render_quantum_frames: None,
            response_budget_frames: NonZeroUsize::new(448),
        }
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

    fn member_count(state: &TestState) -> usize {
        state
            .root
            .topology()
            .expect("the host topology remains valid")
            .members()
            .len()
    }

    fn host_grid(state: &TestState) -> BeatGridSnapshot {
        state.root.snapshot()
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

    fn deck_by_player_id(state: &TestState, player_id: u64) -> &Deck<TestPools> {
        let index = state
            .graph
            .index_by_player(player_id)
            .expect("the player has a registered deck");
        state
            .graph
            .deck(index)
            .expect("the registered deck is present")
    }

    #[kithara::test]
    fn registration_projects_the_canonical_member_grid() {
        route_loss(RouteLossProbe::reset);
        let mut state = test_state(start_route_loss_stream);
        let host_id = state.root.id();

        let player_id = register_player(&mut state);
        let registered = state.root.topology().expect("the host topology is valid");
        let deck = deck_by_player_id(&state, player_id);
        assert_eq!(registered.members().len(), 1);
        assert_eq!(registered.members()[0].grid().id(), deck.grid_id);
        assert!(registered.members()[0].group_topology().is_some());

        assert!(matches!(
            run_cmd(&mut state, start_command(player_id),),
            Reply::Ok
        ));
        assert!(deck_by_player_id(&state, player_id).started);
        let started = state
            .root
            .topology()
            .expect("the host topology remains valid");
        assert_eq!(started.stamp(), registered.stamp());
        // The stream may open a new session epoch at any time; each deck
        // grid descends onto the host's axis without a topology change, so
        // members are compared by identity and by the axis they follow.
        let identity = |topology: &SyncGroupSnapshot| {
            topology
                .members()
                .iter()
                .map(|member| {
                    assert_eq!(member.grid().axis(), topology.group_grid().axis());
                    (
                        member.grid().id(),
                        member.group_topology().map(SyncGroupSnapshot::stamp),
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(identity(&started), identity(&registered));

        assert!(matches!(
            run_cmd(&mut state, Cmd::UnregisterPlayer { player_id }),
            Reply::Ok
        ));

        assert_eq!(state.root.id(), host_id);
        let retained = state
            .root
            .topology()
            .expect("the canonical member outlives its graph projection");
        assert_eq!(retained.stamp(), started.stamp());
        assert_eq!(identity(&retained), identity(&started));
        assert_eq!(deck_count(&state), 0);
    }

    #[kithara::test]
    fn registration_rejects_a_player_before_canonical_attachment() {
        let mut state = test_state(start_route_loss_stream);
        let grid_id = kithara_warp::BeatGridId::allocate().expect("fixture player grid id");

        let reply = run_cmd(&mut state, register_command(grid_id));

        assert!(matches!(reply, Reply::Err(SessionError::Graph(_))));
        assert_eq!(member_count(&state), 0);
        assert_eq!(deck_count(&state), 0);
    }

    #[kithara::test]
    fn duplicate_graph_projection_is_rejected() {
        let mut state = test_state(start_route_loss_stream);
        let grid_id = attach_player(&mut state);
        let command = || register_command(grid_id);

        assert!(matches!(
            run_cmd(&mut state, command()),
            Reply::PlayerRegistered(_)
        ));
        let next_player_id = state.next_player_id;
        assert!(matches!(
            run_cmd(&mut state, command()),
            Reply::Err(SessionError::Graph(_))
        ));

        assert_eq!(state.next_player_id, next_player_id);
        assert_eq!(member_count(&state), 1);
        assert_eq!(deck_count(&state), 1);
    }

    #[kithara::test]
    fn detach_is_rejected_while_the_graph_projection_is_live() {
        let mut state = test_state(start_route_loss_stream);
        let grid_id = attach_player(&mut state);
        let Reply::PlayerRegistered(registered) = run_cmd(&mut state, register_command(grid_id))
        else {
            panic!("fixture player is registered")
        };
        let player_id = registered.id;
        let detach = |state: &TestState| SyncOperation::Topology {
            base: state.root.topology().expect("fixture topology").stamp(),
            operations: Box::new([TopologyOperation::Detach { member: grid_id }]),
        };

        let operation = detach(&state);
        let HostReply::Admission(Err(rejected)) =
            run_host_cmd(&mut state, HostCmd::Sync(SyncCmd::Transact(operation)))
        else {
            panic!("live graph projection rejects canonical detach")
        };
        let (error, _) = <(SyncError, SyncOperation<PlayerMember>)>::from(rejected);
        assert_eq!(
            error,
            SyncError::CapabilityUnavailable {
                capability: SyncCapability::Topology,
            }
        );
        assert_eq!(member_count(&state), 1);
        assert_eq!(deck_count(&state), 1);

        assert!(matches!(
            run_cmd(&mut state, Cmd::UnregisterPlayer { player_id }),
            Reply::Ok
        ));
        let operation = detach(&state);
        assert!(matches!(
            run_host_cmd(&mut state, HostCmd::Sync(SyncCmd::Transact(operation))),
            HostReply::Admission(Ok(kithara_sync::SyncAdmission::TopologyChanged { .. }))
        ));
        assert_eq!(member_count(&state), 0);
        assert_eq!(deck_count(&state), 0);
    }

    #[kithara::test]
    fn owner_side_topology_commands_resolve_the_base_when_executed() {
        let mut state = test_state(start_route_loss_stream);
        let first = attach_player(&mut state);
        let second = attach_player(&mut state);
        let before = state.root.topology().expect("fixture topology").stamp();
        let detach = |member| {
            HostCmd::Sync(SyncCmd::TransactCurrent(Box::new([
                TopologyOperation::Detach { member },
            ])))
        };

        assert!(matches!(
            run_host_cmd(&mut state, detach(first)),
            HostReply::Admission(Ok(kithara_sync::SyncAdmission::TopologyChanged { .. }))
        ));
        let after_first = state.root.topology().expect("updated topology").stamp();
        assert_ne!(after_first, before);
        assert!(matches!(
            run_host_cmd(&mut state, detach(second)),
            HostReply::Admission(Ok(kithara_sync::SyncAdmission::TopologyChanged { .. }))
        ));

        let after_second = state.root.topology().expect("updated topology");
        assert_ne!(after_second.stamp(), after_first);
        assert!(after_second.members().is_empty());
        assert_eq!(state.root_view.topology(), Ok(after_second));
    }

    #[kithara::test]
    fn root_view_publishes_the_canonical_topology() {
        let mut state = test_state(start_route_loss_stream);
        let grid_id = attach_player(&mut state);

        let topology = state.root.topology().expect("canonical topology");
        let published = state.root_view.topology().expect("published topology");

        assert_eq!(published, topology);
        assert_eq!(published.members().len(), 1);
        assert_eq!(published.members()[0].grid().id(), grid_id);
    }

    #[kithara::test]
    fn exhausted_player_identity_preserves_the_canonical_root() {
        let mut state = test_state(start_route_loss_stream);
        let grid_id = attach_player(&mut state);
        let topology = state.root.topology().expect("fixture topology");
        state.next_player_id = u64::MAX;

        let reply = run_cmd(&mut state, register_command(grid_id));

        assert!(matches!(reply, Reply::Err(SessionError::PlayerIdExhausted)));
        assert_eq!(state.next_player_id, u64::MAX);
        assert_eq!(deck_count(&state), 0);
        assert_eq!(state.root.topology().expect("fixture topology"), topology);
        assert!(state.reserved_session_grid.is_some());
    }

    #[kithara::test]
    fn sample_rate_query_separates_the_measured_stream_from_the_request() {
        route_loss(RouteLossProbe::reset);

        let mut state = test_state(start_route_loss_stream);
        let Reply::SampleRate(before) = run_cmd(&mut state, Cmd::QuerySampleRate) else {
            panic!("the sample-rate query answers with a sample rate");
        };
        assert_eq!(
            before.measured, None,
            "a session with no stream has measured nothing"
        );
        assert_eq!(
            before.output(),
            TestState::DEFAULT_SAMPLE_RATE,
            "until a stream exists the resampler is built for the requested rate"
        );

        let player_id = register_player(&mut state);
        assert!(matches!(
            run_cmd(&mut state, Cmd::QuerySampleRate),
            Reply::SampleRate(SessionSampleRate {
                measured: None,
                requested: TestState::DEFAULT_SAMPLE_RATE,
                ..
            })
        ));
        configure_sample_rate(&mut state, 48_000);
        assert!(matches!(
            run_cmd(&mut state, Cmd::QuerySampleRate),
            Reply::SampleRate(SessionSampleRate {
                measured: None,
                requested: 48_000,
                ..
            })
        ));
        start_player_cmd(&mut state, player_id);
        assert!(matches!(
            run_cmd(&mut state, Cmd::QuerySampleRate),
            Reply::SampleRate(SessionSampleRate {
                measured: Some(48_000),
                requested: 48_000,
                ..
            })
        ));
    }

    #[kithara::test]
    fn a_sample_rate_set_while_idle_is_the_rate_play_starts_the_stream_at() {
        route_loss(RouteLossProbe::reset);

        let mut state = test_state(start_route_loss_stream);
        let rate = NonZeroU32::new(48_000).expect("48000 is not zero");
        configure_sample_rate(&mut state, rate.get());
        let player_id = register_player(&mut state);
        start_player_cmd(&mut state, player_id);

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
    fn stream_shape_query_prefers_measurement_over_an_explicit_request() {
        route_loss(RouteLossProbe::reset);

        let mut state = test_state(start_route_loss_stream);
        assert!(matches!(
            run_cmd(&mut state, Cmd::QueryStreamShape),
            Reply::StreamShape(None)
        ));

        state.requested_max_block_frames = NonZeroU32::new(128);
        let Reply::StreamShape(Some(requested)) = run_cmd(&mut state, Cmd::QueryStreamShape) else {
            panic!("the explicit output block is available before stream start")
        };
        assert_eq!(requested.max_block_frames.get(), 128);
        assert_eq!(requested.sample_rate.get(), TestState::DEFAULT_SAMPLE_RATE);

        let player_id = register_player(&mut state);
        assert!(matches!(
            run_cmd(&mut state, start_command(player_id),),
            Reply::Ok
        ));
        let Reply::StreamShape(Some(measured)) = run_cmd(&mut state, Cmd::QueryStreamShape) else {
            panic!("the running stream reports its measured output shape")
        };
        assert_eq!(measured.max_block_frames.get(), 512);
        assert_eq!(measured.sample_rate.get(), TestState::DEFAULT_SAMPLE_RATE);
        assert_eq!(state.root_view.stream_shape(), Some(measured));
        configure_sample_rate(&mut state, 48_000);
        assert_eq!(
            state
                .root_view
                .stream_shape()
                .expect("published shape")
                .sample_rate
                .get(),
            48_000
        );
        assert!(matches!(
            run_cmd(&mut state, Cmd::StopPlayer { player_id }),
            Reply::Ok
        ));
        let stopped = state
            .root_view
            .stream_shape()
            .expect("configured shape after stop");
        assert_eq!(stopped.max_block_frames.get(), 128);
        assert_eq!(stopped.sample_rate.get(), 48_000);
    }

    #[kithara::test]
    fn measured_output_block_rejects_player_before_graph_start() {
        route_loss(RouteLossProbe::reset);

        let mut state = test_state(start_route_loss_stream);
        state.requested_max_block_frames = NonZeroU32::new(128);
        let player_id = register_player(&mut state);
        let command = Cmd::StartPlayer {
            player_id,
            master_volume: 1.0,
            render_quantum_frames: NonZeroUsize::new(64),
            response_budget_frames: NonZeroUsize::new(441),
        };

        assert!(matches!(
            run_cmd(&mut state, command),
            Reply::Err(SessionError::ResponseBudgetExceeded {
                max_block_frames: 512,
                render_quantum_frames: 64,
                required_frames: 639,
                budget_frames: 441,
            })
        ));
        assert!(!deck_by_player_id(&state, player_id).started);
    }

    #[kithara::test]
    fn explicit_audio_route_invalidation_restarts_stream_without_backend_error() {
        route_loss(RouteLossProbe::reset);

        let mut state = test_state(start_route_loss_stream);
        let player_id = register_player(&mut state);

        assert!(matches!(
            run_cmd(&mut state, start_command(player_id),),
            Reply::Ok
        ));
        assert!(matches!(
            run_cmd(&mut state, Cmd::QuerySampleRate),
            Reply::SampleRate(SessionSampleRate {
                measured: Some(44_100),
                requested: 44_100,
                ..
            })
        ));
        assert!(matches!(
            run_cmd(&mut state, Cmd::AllocateSlot { player_id }),
            Reply::SlotAllocated(..)
        ));
        assert_eq!(
            route_loss(|probe| probe.start_count.load(Ordering::SeqCst)),
            1
        );
        let before_route = host_grid(&state);

        assert!(matches!(
            run_cmd(
                &mut state,
                Cmd::InvalidateAudioRoute {
                    reason: String::from("oldDeviceUnavailable"),
                },
            ),
            Reply::Ok
        ));

        assert_eq!(
            route_loss(|probe| probe.start_count.load(Ordering::SeqCst)),
            2,
            "explicit platform route invalidation must restart the audio stream"
        );
        assert_route_boundary(&before_route, &host_grid(&state));
        let first_boundary = host_grid(&state);
        assert!(matches!(
            run_cmd(
                &mut state,
                Cmd::InvalidateAudioRoute {
                    reason: String::from("newDeviceAvailable"),
                },
            ),
            Reply::Ok
        ));
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
            deck(&state, 0).started,
            "route invalidation must keep the player graph logically started"
        );
        assert_eq!(
            deck(&state, 0).slots.len(),
            1,
            "route invalidation must not drop active slots"
        );
        assert!(matches!(
            run_cmd(&mut state, Cmd::AllocateSlot { player_id }),
            Reply::SlotAllocated(..)
        ));
        assert_eq!(
            deck(&state, 0).slots.len(),
            2,
            "session must accept future slots after explicit route restart"
        );
        assert!(!state.stream_needs_restart);
    }

    #[kithara::test]
    fn unexpected_stream_stop_restarts_stream_without_dropping_player_graph_or_future_slots() {
        route_loss(RouteLossProbe::reset);

        let mut state = test_state(start_route_loss_stream);
        let player_id = register_player(&mut state);

        assert!(matches!(
            run_cmd(&mut state, start_command(player_id),),
            Reply::Ok
        ));
        assert!(state.ctx.is_some());
        assert!(deck(&state, 0).started);
        assert_eq!(
            route_loss(|probe| probe.start_count.load(Ordering::SeqCst)),
            1
        );
        assert!(matches!(
            run_cmd(&mut state, Cmd::AllocateSlot { player_id }),
            Reply::SlotAllocated(..)
        ));
        assert_eq!(deck(&state, 0).slots.len(), 1);
        let before_route = host_grid(&state);

        state.stream = None;
        assert!(matches!(run_cmd(&mut state, Cmd::Tick), Reply::Ok));

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
            deck(&state, 0).started,
            "player graph must remain logically started after stream restart"
        );
        assert_eq!(
            deck(&state, 0).slots.len(),
            1,
            "active slot graph must survive stream restart"
        );
        assert!(matches!(
            run_cmd(&mut state, Cmd::AllocateSlot { player_id }),
            Reply::SlotAllocated(..)
        ));
        assert_eq!(
            deck(&state, 0).slots.len(),
            2,
            "session must accept a future slot after route-loss reinit"
        );
        assert!(!state.stream_needs_restart);
    }

    #[kithara::test]
    fn stream_loss_seen_while_draining_host_commands_restarts_the_stream() {
        route_loss(RouteLossProbe::reset);

        let mut state = test_state(start_route_loss_stream);
        let player_id = register_player(&mut state);

        assert!(matches!(
            run_cmd(&mut state, start_command(player_id)),
            Reply::Ok
        ));
        assert_eq!(
            route_loss(|probe| probe.start_count.load(Ordering::SeqCst)),
            1
        );

        let (_tx, rx) = mpsc::channel::<HostCmdMsg<TestPools>>();

        state.stream = None;
        drain_host_channel(&mut state, &rx, |_| {});

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
        assert!(matches!(tick_session(&mut state), Reply::Ok));
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
        let (_tx, rx) = mpsc::channel::<HostCmdMsg<TestPools>>();

        drain_host_channel(&mut state, &rx, |_| {});

        assert!(state.root_view.grid().revision() > before.revision());
    }

    #[kithara::test]
    fn failed_stream_restart_is_retried_on_next_tick() {
        route_loss(RouteLossProbe::reset);

        let mut state = test_state(start_route_loss_stream);
        let player_id = register_player(&mut state);

        assert!(matches!(
            run_cmd(&mut state, start_command(player_id),),
            Reply::Ok
        ));
        assert_eq!(
            route_loss(|probe| probe.start_count.load(Ordering::SeqCst)),
            1
        );
        let before_route = host_grid(&state);

        state.stream = None;
        route_loss(|probe| probe.fail_next_start.store(true, Ordering::SeqCst));
        match run_cmd(&mut state, Cmd::Tick) {
            Reply::Err(err) => assert!(
                matches!(err, SessionError::RestartFailed { .. }),
                "restart failure must be surfaced, got {err:?}"
            ),
            _ => panic!("failed restart must return Reply::Err"),
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

        assert!(matches!(run_cmd(&mut state, Cmd::Tick), Reply::Ok));
        assert_eq!(
            route_loss(|probe| probe.start_count.load(Ordering::SeqCst)),
            3,
            "next tick must retry the stream restart"
        );
        let retried = host_grid(&state);
        assert_eq!(retried.stamp(), boundary.stamp());
        assert_eq!(retried.axis(), boundary.axis());
        assert!(!state.stream_needs_restart);
        assert!(deck(&state, 0).started);
    }

    fn start_player_cmd(state: &mut TestState, player_id: u64) {
        assert!(matches!(
            run_cmd(&mut *state, start_command(player_id),),
            Reply::Ok
        ));
    }

    fn master_volume_of(state: &TestState, player_id: u64) -> f32 {
        state
            .graph
            .decks()
            .find(|player| player.player_id == player_id)
            .expect("player present")
            .master_volume
    }

    fn apply_player_mix(
        state: &mut TestState,
        levels: impl IntoIterator<Item = (u64, f32)>,
    ) -> HostReply {
        let levels = levels
            .into_iter()
            .map(|(player_id, level)| {
                HostLevel::new(deck_by_player_id(state, player_id).grid_id, level)
            })
            .collect();
        run_host_cmd(state, HostCmd::ApplyMix { levels })
    }

    #[kithara::test]
    fn host_mix_before_registration_becomes_the_start_level() {
        route_loss(RouteLossProbe::reset);

        let mut state = test_state(start_route_loss_stream);
        let grid_id = attach_player(&mut state);
        assert!(matches!(
            run_host_cmd(
                &mut state,
                HostCmd::ApplyMix {
                    levels: Box::new([HostLevel::new(grid_id, 0.4)]),
                },
            ),
            HostReply::Ok
        ));
        let Reply::PlayerRegistered(registered) = run_cmd(&mut state, register_command(grid_id))
        else {
            panic!("player registration must succeed")
        };
        let player_id = registered.id;

        start_player_cmd(&mut state, player_id);

        assert_eq!(master_volume_of(&state, player_id), 0.4);
        assert_eq!(
            deck_by_player_id(&state, player_id)
                .master_volume_memo
                .as_ref()
                .expect("started player has a volume node")
                .volume,
            master_gain(0.4),
        );
    }

    #[kithara::test]
    fn host_mix_updates_one_two_and_four_players_together() {
        route_loss(RouteLossProbe::reset);

        let mut state = test_state(start_route_loss_stream);
        let ids: Vec<u64> = (0..4).map(|_| register_player(&mut state)).collect();
        for &id in &ids {
            start_player_cmd(&mut state, id);
        }

        assert!(matches!(
            apply_player_mix(&mut state, [(ids[0], 0.1)]),
            HostReply::Ok
        ));
        assert_eq!(master_volume_of(&state, ids[0]), 0.1);

        assert!(matches!(
            apply_player_mix(&mut state, [(ids[1], 0.2), (ids[2], 0.3)]),
            HostReply::Ok
        ));
        assert_eq!(master_volume_of(&state, ids[1]), 0.2);
        assert_eq!(master_volume_of(&state, ids[2]), 0.3);
        assert_eq!(master_volume_of(&state, ids[3]), 1.0);

        assert!(matches!(
            apply_player_mix(
                &mut state,
                [(ids[0], 0.4), (ids[1], 0.5), (ids[2], 0.6), (ids[3], 0.7),],
            ),
            HostReply::Ok
        ));
        assert_eq!(master_volume_of(&state, ids[0]), 0.4);
        assert_eq!(master_volume_of(&state, ids[3]), 0.7);
    }

    #[kithara::test]
    fn host_mix_rejects_duplicate_player_without_mutation() {
        route_loss(RouteLossProbe::reset);

        let mut state = test_state(start_route_loss_stream);
        let id = register_player(&mut state);
        start_player_cmd(&mut state, id);

        assert!(matches!(
            apply_player_mix(&mut state, [(id, 0.3), (id, 0.4)]),
            HostReply::Err(PlayError::MixDuplicatePlayer)
        ));
        assert_eq!(master_volume_of(&state, id), 1.0);
    }

    #[kithara::test]
    fn host_mix_rejects_invalid_level_without_mutation() {
        route_loss(RouteLossProbe::reset);

        let mut state = test_state(start_route_loss_stream);
        let a = register_player(&mut state);
        let b = register_player(&mut state);
        start_player_cmd(&mut state, a);
        start_player_cmd(&mut state, b);

        for bad in [f32::NAN, f32::INFINITY, 1.5, -0.1] {
            assert!(matches!(
                apply_player_mix(&mut state, [(a, 0.5), (b, bad)]),
                HostReply::Err(PlayError::MixLevel { .. })
            ));
            assert_eq!(
                master_volume_of(&state, a),
                1.0,
                "level {bad} leaked a mutation"
            );
            assert_eq!(master_volume_of(&state, b), 1.0);
        }
    }

    #[kithara::test]
    fn host_mix_rejects_foreign_member_leaving_known_unchanged() {
        route_loss(RouteLossProbe::reset);

        let mut state = test_state(start_route_loss_stream);
        let known = register_player(&mut state);
        start_player_cmd(&mut state, known);
        let known_grid = deck_by_player_id(&state, known).grid_id;
        let unknown_grid = kithara_warp::BeatGridId::allocate().expect("foreign fixture grid id");

        assert!(matches!(
            run_host_cmd(
                &mut state,
                HostCmd::ApplyMix {
                    levels: Box::new([
                        HostLevel::new(known_grid, 0.2),
                        HostLevel::new(unknown_grid, 0.3),
                    ]),
                },
            ),
            HostReply::Err(PlayError::MixForeignSession)
        ));
        assert_eq!(master_volume_of(&state, known), 1.0);
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
        let id = register_player(&mut state);
        start_player_cmd(&mut state, id);

        let drops = Arc::new(AtomicU64::new(0));
        let mut outputs = OutputGroup::new();
        outputs.push(mix_tap_writer(&drops));
        outputs.push(mix_tap_writer(&drops));
        assert!(matches!(
            run_host_cmd(
                &mut state,
                HostCmd::AttachOutputs {
                    tap: Tap::Master,
                    outputs,
                },
            ),
            HostReply::Ok
        ));
        assert!(
            matches!(state.taps.slot(Tap::Master), Some(TapSlot::Installed(_))),
            "a tap armed on a running session reaches the graph at once"
        );

        let mut second = OutputGroup::new();
        second.push(mix_tap_writer(&drops));
        assert!(
            matches!(
                run_host_cmd(
                    &mut state,
                    HostCmd::AttachOutputs {
                        tap: Tap::Master,
                        outputs: second,
                    },
                ),
                HostReply::Err(PlayError::Session(SessionError::TapActive))
            ),
            "a second consumer must be rejected instead of silently replacing the first"
        );

        let mut beside = OutputGroup::new();
        beside.push(mix_tap_writer(&drops));
        assert!(
            matches!(
                run_host_cmd(
                    &mut state,
                    HostCmd::AttachOutputs {
                        tap: Tap::Output,
                        outputs: beside,
                    },
                ),
                HostReply::Ok
            ),
            "the output tap takes its own group beside the master tap"
        );

        assert!(matches!(
            run_cmd(&mut state, Cmd::StopPlayer { player_id: id }),
            Reply::Ok
        ));
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
        let id = register_player(&mut state);
        start_player_cmd(&mut state, id);
        assert!(matches!(
            run_host_cmd(
                &mut state,
                HostCmd::Configure {
                    change: HostSettingsChange::Metronome(MetronomeConfigChange::Enabled(true)),
                    at: When::Next,
                },
            ),
            HostReply::Ok
        ));
        assert!(
            !state.settings.config().metronome().enabled(),
            "the change waits for a block that never renders"
        );

        assert!(matches!(
            run_cmd(&mut state, Cmd::StopPlayer { player_id: id }),
            Reply::Ok
        ));
        assert!(
            state.settings.config().metronome().enabled(),
            "the teardown folds the change in flight into the settings"
        );

        start_player_cmd(&mut state, id);
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
    fn configure_next(state: &mut TestState, change: HostSettingsChange) -> HostReply {
        run_host_cmd(
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
        let id = register_player(&mut state);
        start_player_cmd(&mut state, id);
        configure_sample_rate(&mut state, 48_000);
        let enable = HostSettingsChange::Metronome(MetronomeConfigChange::Enabled(true));
        assert!(matches!(configure_next(&mut state, enable), HostReply::Ok));
        let mut clock = 0;
        render_left(&mut state, &mut clock, 1);

        route_loss(|probe| probe.fail_next_start.store(true, Ordering::SeqCst));
        state.stream = None;
        assert!(matches!(
            run_cmd(&mut state, Cmd::Tick),
            Reply::Err(SessionError::RestartFailed { .. })
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
        let id = register_player(&mut state);
        start_player_cmd(&mut state, id);
        let mut clock = 0;
        let mut applied = state.settings.config().metronome().level();
        for step in 0..2 * host_queue_capacity() {
            let (level, change) = level_change(step);
            assert!(
                matches!(configure_next(&mut state, change), HostReply::Ok),
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
        let id = register_player(&mut state);
        start_player_cmd(&mut state, id);
        let capacity = host_queue_capacity();
        for step in 0..capacity {
            assert!(
                matches!(
                    configure_next(&mut state, level_change(step).1),
                    HostReply::Ok
                ),
                "change {step} fits the queue"
            );
        }
        let before = *state.settings.config();

        assert!(matches!(
            configure_next(&mut state, level_change(capacity).1),
            HostReply::Err(PlayError::Session(SessionError::HostQueueFull))
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
            HostReply::Ok
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
        let id = register_player(&mut state);
        start_player_cmd(&mut state, id);
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
            run_host_cmd(
                &mut state,
                HostCmd::Configure {
                    change: HostSettingsChange::Ducking(SessionDuckingMode::Hard),
                    at: When::Next,
                },
            ),
            HostReply::Ok
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
    fn a_ducking_mode_from_a_player_session_is_the_host_setting() {
        let mut state = test_state(start_route_loss_stream);

        assert!(matches!(
            run_cmd(
                &mut state,
                Cmd::SetSessionDucking {
                    mode: SessionDuckingMode::Soft,
                },
            ),
            Reply::Ok
        ));

        assert_eq!(
            state.settings.config().ducking(),
            SessionDuckingMode::Soft,
            "with no render graph the setting changes at once"
        );
    }

    #[kithara::test]
    fn a_player_attached_after_an_idle_teardown_stops_through_the_next_one() {
        route_loss(RouteLossProbe::reset);

        let mut state = test_state(start_route_loss_stream);
        let first = register_player(&mut state);
        start_player_cmd(&mut state, first);
        assert!(matches!(
            run_cmd(&mut state, Cmd::StopPlayer { player_id: first }),
            Reply::Ok
        ));

        let second = register_player(&mut state);
        start_player_cmd(&mut state, second);
        match run_cmd(&mut state, Cmd::StopPlayer { player_id: second }) {
            Reply::Ok => {}
            Reply::Err(error) => {
                panic!("a player that joined after a route boundary must follow the next: {error}")
            }
            _ => panic!("stop returned an unexpected reply"),
        }
    }

    #[kithara::test]
    fn session_output_has_exactly_one_limiter_rebuilt_on_route_recreate() {
        route_loss(RouteLossProbe::reset);

        let mut state = test_state(start_route_loss_stream);
        let id = register_player(&mut state);
        start_player_cmd(&mut state, id);
        assert!(
            state.session_limiter_node_id.is_some(),
            "limiter node exists after start"
        );

        assert!(matches!(
            run_cmd(&mut state, Cmd::StopPlayer { player_id: id }),
            Reply::Ok
        ));
        assert!(state.session_limiter_node_id.is_none());
        assert!(state.session_output_node_id.is_none());

        start_player_cmd(&mut state, id);
        assert!(
            state.session_limiter_node_id.is_some(),
            "route recreate rebuilds the limiter node"
        );
    }
}
