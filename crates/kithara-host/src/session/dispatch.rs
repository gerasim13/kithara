use firewheel::{FirewheelCtx, backend::AudioBackend, error::UpdateError};
use kithara_play::PlayError;
use kithara_warp::{SyncCapability, SyncGroup, SyncOperation, SyncRejected, TopologyOperation};
use tracing::{debug, trace, warn};

use super::{
    graph::{controls, lifecycle, player_index, slots, tap},
    protocol::{
        Cmd, HostCmd, HostReply, PlayerId, PlayerLevel, Reply, SessionError, SessionSampleRate,
        SyncCmd,
    },
    state::{SessionState, register_player},
    transport,
};
use crate::api::HostLevel;

pub(crate) fn run_host_cmd<B: AudioBackend>(
    state: &mut SessionState<B>,
    cmd: HostCmd,
) -> HostReply {
    match cmd {
        HostCmd::Play(cmd) => HostReply::Play(run_cmd(state, cmd)),
        HostCmd::Sync(cmd) => run_sync_cmd(state, cmd),
        HostCmd::ApplyMix { levels } => {
            apply_mix(state, &levels).map_or_else(HostReply::Err, |()| HostReply::Ok)
        }
        HostCmd::Shutdown => HostReply::Ok,
    }
}

fn run_sync_cmd<B: AudioBackend>(state: &mut SessionState<B>, cmd: SyncCmd) -> HostReply {
    let operation = match cmd {
        SyncCmd::Transact(operation) => operation,
        SyncCmd::TransactCurrent(operations) => {
            let topology = match state.root.topology() {
                Ok(topology) => topology,
                Err(error) => return HostReply::Err(SessionError::from(error).into()),
            };
            SyncOperation::Topology {
                base: topology.stamp(),
                operations,
            }
        }
        SyncCmd::Acknowledge(applied) => {
            let result = state.root.acknowledge(applied);
            if result.is_ok() {
                state.publish_root();
            }
            return HostReply::Acknowledged(result);
        }
    };
    let result = transact_root(state, operation);
    if result.is_ok() {
        state.publish_root();
    }
    HostReply::Admission(result)
}

fn transact_root<B: AudioBackend>(
    state: &mut SessionState<B>,
    operation: SyncOperation<kithara_play::player::PlayerMember>,
) -> Result<kithara_warp::SyncAdmission, SyncRejected<kithara_play::player::PlayerMember>> {
    if topology_conflicts_with_graph(state, &operation) {
        return Err(SyncRejected::new(
            kithara_warp::SyncError::CapabilityUnavailable {
                capability: SyncCapability::Topology,
            },
            operation,
        ));
    }
    state.root.transact(operation)
}

fn topology_conflicts_with_graph<B: AudioBackend>(
    state: &SessionState<B>,
    operation: &SyncOperation<kithara_play::player::PlayerMember>,
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

pub(crate) fn run_cmd<B: AudioBackend>(state: &mut SessionState<B>, cmd: Cmd) -> Reply {
    match cmd {
        Cmd::RegisterPlayer {
            grid_id,
            bus,
            eq_layout,
            pcm_pool,
            sample_rate,
        } => match register_player(state, grid_id, bus, eq_layout, pcm_pool, sample_rate) {
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
            sample_rate,
        } => match lifecycle::start_player(state, player_id, sample_rate, master_volume) {
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
        Cmd::EnableMixTap { writer } => match tap::enable(state, writer) {
            Ok(()) => Reply::Ok,
            Err(err) => Reply::Err(err),
        },
        Cmd::DisableMixTap => {
            tap::disable(state);
            Reply::Ok
        }
        Cmd::SetSessionDucking { mode } => {
            controls::set_session_ducking(state, mode);
            Reply::Ok
        }
        Cmd::SessionDucking => Reply::SessionDucking(state.session_ducking),
        Cmd::SetSessionTempo { tempo } => match transport::set_tempo(state, tempo) {
            Ok(()) => Reply::Ok,
            Err(err) => Reply::Err(err),
        },
        Cmd::SetSessionPlaying { playing } => match transport::set_playing(state, playing) {
            Ok(()) => Reply::Ok,
            Err(err) => Reply::Err(err),
        },
        Cmd::SeekSession { target } => match transport::seek(state, target) {
            Ok(()) => Reply::Ok,
            Err(err) => Reply::Err(err),
        },
        Cmd::QuerySessionTransport => match transport::snapshot(state) {
            Ok(snapshot) => Reply::SessionTransport(snapshot),
            Err(err) => Reply::Err(err),
        },
        Cmd::InvalidateAudioRoute { reason } => invalidate_audio_route(state, &reason),
        Cmd::QuerySampleRate => {
            let measured = state
                .ctx
                .as_ref()
                .and_then(FirewheelCtx::stream_info)
                .map(|info| info.sample_rate.get());
            trace_stream_info(state, "query-sample-rate");
            Reply::SampleRate(SessionSampleRate::new(measured, state.sample_rate_hint))
        }
        Cmd::Tick => tick_session(state),
    }
}

pub(super) fn tick_session<B: AudioBackend>(state: &mut SessionState<B>) -> Reply {
    if state.stream_needs_restart {
        match restart_stream(state, state.sample_rate_hint) {
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

    let update = state.ctx.as_mut().map(FirewheelCtx::update);
    if let Some(Err(err)) = update {
        return handle_update_error(state, err);
    }
    Reply::Ok
}

fn unregister_player<B: AudioBackend>(
    state: &mut SessionState<B>,
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

fn apply_mix<B: AudioBackend>(
    state: &mut SessionState<B>,
    levels: &[HostLevel],
) -> Result<(), PlayError> {
    let mut projected = Vec::with_capacity(levels.len());
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
            member.dispatch(|player| player.set_host_level(level));
        });
        if updated.is_none() {
            return Err(PlayError::MixForeignSession);
        }
    }
    Ok(())
}

pub(super) fn handle_update_error<B: AudioBackend>(
    state: &mut SessionState<B>,
    err: UpdateError<B::StreamError>,
) -> Reply {
    match err {
        UpdateError::StreamStoppedUnexpectedly(reason) => {
            state.stream_needs_restart = true;
            warn!(
                ?reason,
                "session stream stopped unexpectedly; restarting audio stream"
            );
            trace!(
                ?reason,
                sample_rate_hint = state.sample_rate_hint,
                "[KITHARA-ROUTE] firewheel update reported stopped stream"
            );
            match restart_stream(state, state.sample_rate_hint) {
                Ok(()) => Reply::Ok,
                Err(restart_err) => Reply::Err(SessionError::RestartFailed {
                    reason: format!("{reason:?}"),
                    r#source: restart_err.to_string(),
                }),
            }
        }
        other => {
            warn!(?other, "[KITHARA-ROUTE] firewheel update failed");
            Reply::Err(SessionError::Graph(format!("{other:?}")))
        }
    }
}

pub(super) fn invalidate_audio_route<B: AudioBackend>(
    state: &mut SessionState<B>,
    reason: &str,
) -> Reply {
    debug!(
        reason,
        ctx_ready = state.ctx.is_some(),
        stream_needs_restart = state.stream_needs_restart,
        "[KITHARA-ROUTE] audio route invalidated"
    );
    if state.ctx.is_none() {
        return Reply::Ok;
    }
    state.stream_needs_restart = true;
    match restart_stream(state, state.sample_rate_hint) {
        Ok(()) => Reply::Ok,
        Err(err) => Reply::Err(SessionError::RestartFailed {
            reason: reason.to_owned(),
            r#source: err.to_string(),
        }),
    }
}

pub(super) fn restart_stream<B: AudioBackend>(
    state: &mut SessionState<B>,
    sample_rate: u32,
) -> Result<(), SessionError> {
    if state.ctx.is_none() {
        return Err(SessionError::NoContext);
    }
    debug!(sample_rate, "[KITHARA-ROUTE] restarting firewheel stream");
    if transport::prepare_route_restart(state, sample_rate)?
        == transport::RouteRestartStatus::Pending
    {
        trace!("[KITHARA-ROUTE] waiting for the previous stream processor to stop");
        return Ok(());
    }
    let fw_ctx = state.ctx.as_mut().ok_or(SessionError::NoContext)?;
    (state.start_stream_fn)(fw_ctx, sample_rate).map_err(SessionError::StreamStart)?;
    state.reserved_session_grid = None;
    state.sample_rate_hint = sample_rate;
    state.stream_needs_restart = false;
    trace_stream_info(state, "restart-stream");
    debug!(
        sample_rate,
        "[KITHARA-ROUTE] firewheel stream restart complete"
    );
    Ok(())
}

pub(super) fn trace_stream_info<B: AudioBackend>(state: &SessionState<B>, context: &'static str) {
    if let Some(info) = state.ctx.as_ref().and_then(FirewheelCtx::stream_info) {
        trace!(
            context,
            sample_rate = info.sample_rate.get(),
            prev_sample_rate = info.prev_sample_rate.get(),
            max_block_frames = info.max_block_frames.get(),
            out_channels = info.num_stream_out_channels,
            output_device_id = %info.output_device_id,
            input_device_id = ?info.input_device_id.as_deref(),
            stream_needs_restart = state.stream_needs_restart,
            "[KITHARA-ROUTE] session stream-info"
        );
    } else {
        trace!(
            context,
            sample_rate_hint = state.sample_rate_hint,
            stream_needs_restart = state.stream_needs_restart,
            "[KITHARA-ROUTE] session stream-info unavailable"
        );
    }
}

#[cfg(test)]
mod tests {
    use std::{
        num::NonZeroU32,
        sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    };

    use firewheel::{FirewheelCtx, StreamInfo, processor::FirewheelProcessor};
    use kithara_bufpool::PcmPool;
    use kithara_events::EventBus;
    use kithara_platform::sync::Arc;
    use kithara_test_utils::kithara;
    use kithara_warp::{BeatGrid, BeatGridSnapshot, BeatGridState, BeatGridUnavailable, MapAxis};
    use ringbuf::{HeapRb, traits::Split};

    use super::*;
    use crate::{
        bridge::MixTapWriter,
        session::{
            protocol::{Cmd, PlayerLevel, Reply, SessionError},
            state::{Deck, MixTap, SessionState},
            testing::{attach_player, state as test_state},
        },
    };

    #[derive(Default)]
    struct RouteLossProbe {
        fail_next_poll: AtomicBool,
        fail_next_start: AtomicBool,
        start_count: AtomicUsize,
    }

    impl RouteLossProbe {
        fn reset(&self) {
            self.start_count.store(0, Ordering::SeqCst);
            self.fail_next_poll.store(false, Ordering::SeqCst);
            self.fail_next_start.store(false, Ordering::SeqCst);
        }
    }

    thread_local! {
        static ROUTE_LOSS: RouteLossProbe = RouteLossProbe::default();
    }

    fn route_loss<R>(f: impl FnOnce(&RouteLossProbe) -> R) -> R {
        ROUTE_LOSS.with(f)
    }

    struct RouteLossBackend {
        _processor: Option<FirewheelProcessor<Self>>,
    }

    #[derive(Clone)]
    struct RouteLossConfig {
        sample_rate: u32,
    }

    impl Default for RouteLossConfig {
        fn default() -> Self {
            Self {
                sample_rate: SessionState::<RouteLossBackend>::DEFAULT_SAMPLE_RATE,
            }
        }
    }

    #[derive(Debug, thiserror::Error)]
    #[error("route lost")]
    struct RouteLossError;

    impl AudioBackend for RouteLossBackend {
        type Config = RouteLossConfig;
        type Enumerator = ();
        type Instant = kithara_platform::time::Instant;
        type StartStreamError = RouteLossError;
        type StreamError = RouteLossError;

        fn delay_from_last_process(
            &self,
            _process_timestamp: Self::Instant,
        ) -> Option<kithara_platform::time::Duration> {
            None
        }

        fn enumerator() -> Self::Enumerator {}

        fn poll_status(&mut self) -> Result<(), Self::StreamError> {
            if route_loss(|probe| probe.fail_next_poll.swap(false, Ordering::SeqCst)) {
                Err(RouteLossError)
            } else {
                Ok(())
            }
        }

        fn set_processor(&mut self, processor: FirewheelProcessor<Self>) {
            self._processor = Some(processor);
        }

        fn start_stream(
            config: Self::Config,
        ) -> Result<(Self, StreamInfo), Self::StartStreamError> {
            route_loss(|probe| probe.start_count.fetch_add(1, Ordering::SeqCst));
            if route_loss(|probe| probe.fail_next_start.swap(false, Ordering::SeqCst)) {
                return Err(RouteLossError);
            }

            let sample_rate = NonZeroU32::new(config.sample_rate).unwrap_or(
                NonZeroU32::new(SessionState::<Self>::DEFAULT_SAMPLE_RATE)
                    .expect("invariant: fixture default sample rate is non-zero"),
            );
            let max_block_frames = NonZeroU32::new(512).ok_or(RouteLossError)?;
            let stream_info = StreamInfo {
                sample_rate,
                sample_rate_recip: 1.0 / f64::from(sample_rate.get()),
                prev_sample_rate: sample_rate,
                max_block_frames,
                num_stream_in_channels: 0,
                num_stream_out_channels: 2,
                input_to_output_latency_seconds: 0.0,
                declick_frames: max_block_frames,
                output_device_id: String::from("route-loss-test"),
                input_device_id: None,
            };
            Ok((Self { _processor: None }, stream_info))
        }
    }

    fn start_route_loss_stream(
        ctx: &mut FirewheelCtx<RouteLossBackend>,
        sample_rate: u32,
    ) -> Result<(), String> {
        ctx.start_stream(RouteLossConfig { sample_rate })
            .map_err(|err| err.to_string())
    }

    fn register_command(grid_id: kithara_warp::BeatGridId, sample_rate: u32) -> Cmd {
        Cmd::RegisterPlayer {
            grid_id,
            bus: EventBus::default(),
            eq_layout: Vec::new(),
            pcm_pool: PcmPool::default(),
            sample_rate,
        }
    }

    fn register_player(state: &mut SessionState<RouteLossBackend>) -> u64 {
        let grid_id = attach_player(state);
        match run_cmd(
            state,
            register_command(
                grid_id,
                SessionState::<RouteLossBackend>::DEFAULT_SAMPLE_RATE,
            ),
        ) {
            Reply::PlayerRegistered(id) => id,
            Reply::Err(err) => panic!("player registration failed: {err}"),
            _ => panic!("player registration returned unexpected reply"),
        }
    }

    fn deck(state: &SessionState<RouteLossBackend>, index: usize) -> &Deck {
        state
            .graph
            .deck(index)
            .expect("the registered deck is present under the host")
    }

    fn deck_count(state: &SessionState<RouteLossBackend>) -> usize {
        state.graph.len()
    }

    fn member_count(state: &SessionState<RouteLossBackend>) -> usize {
        state
            .root
            .topology()
            .expect("the host topology remains valid")
            .members()
            .len()
    }

    fn host_grid(state: &SessionState<RouteLossBackend>) -> BeatGridSnapshot {
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

    fn deck_by_player_id(state: &SessionState<RouteLossBackend>, player_id: u64) -> &Deck {
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
            run_cmd(
                &mut state,
                Cmd::StartPlayer {
                    player_id,
                    sample_rate: SessionState::<RouteLossBackend>::DEFAULT_SAMPLE_RATE,
                    master_volume: 1.0,
                },
            ),
            Reply::Ok
        ));
        assert!(deck_by_player_id(&state, player_id).started);
        let started = state
            .root
            .topology()
            .expect("the host topology remains valid");
        assert_eq!(started.stamp(), registered.stamp());
        assert_eq!(started.members(), registered.members());

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
        assert_eq!(retained.members(), started.members());
        assert_eq!(deck_count(&state), 0);
    }

    #[kithara::test]
    fn registration_rejects_a_player_before_canonical_attachment() {
        let mut state = test_state(start_route_loss_stream);
        let grid_id = kithara_warp::BeatGridId::allocate().expect("fixture player grid id");

        let reply = run_cmd(
            &mut state,
            register_command(
                grid_id,
                SessionState::<RouteLossBackend>::DEFAULT_SAMPLE_RATE,
            ),
        );

        assert!(matches!(reply, Reply::Err(SessionError::Graph(_))));
        assert_eq!(member_count(&state), 0);
        assert_eq!(deck_count(&state), 0);
    }

    #[kithara::test]
    fn duplicate_graph_projection_is_rejected() {
        let mut state = test_state(start_route_loss_stream);
        let grid_id = attach_player(&mut state);
        let command = || {
            register_command(
                grid_id,
                SessionState::<RouteLossBackend>::DEFAULT_SAMPLE_RATE,
            )
        };

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
        let Reply::PlayerRegistered(player_id) = run_cmd(
            &mut state,
            register_command(
                grid_id,
                SessionState::<RouteLossBackend>::DEFAULT_SAMPLE_RATE,
            ),
        ) else {
            panic!("fixture player is registered")
        };
        let detach = |state: &SessionState<RouteLossBackend>| SyncOperation::Topology {
            base: state.root.topology().expect("fixture topology").stamp(),
            operations: Box::new([TopologyOperation::Detach { member: grid_id }]),
        };

        let operation = detach(&state);
        let HostReply::Admission(Err(rejected)) =
            run_host_cmd(&mut state, HostCmd::Sync(SyncCmd::Transact(operation)))
        else {
            panic!("live graph projection rejects canonical detach")
        };
        let (error, _) = <(
            kithara_warp::SyncError,
            SyncOperation<kithara_play::player::PlayerMember>,
        )>::from(rejected);
        assert_eq!(
            error,
            kithara_warp::SyncError::CapabilityUnavailable {
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
            HostReply::Admission(Ok(kithara_warp::SyncAdmission::TopologyChanged { .. }))
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
            HostReply::Admission(Ok(kithara_warp::SyncAdmission::TopologyChanged { .. }))
        ));
        let after_first = state.root.topology().expect("updated topology").stamp();
        assert_ne!(after_first, before);
        assert!(matches!(
            run_host_cmd(&mut state, detach(second)),
            HostReply::Admission(Ok(kithara_warp::SyncAdmission::TopologyChanged { .. }))
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
    fn invalid_registration_preserves_the_canonical_root() {
        let mut state = test_state(start_route_loss_stream);
        let grid_id = attach_player(&mut state);
        let next_player_id = state.next_player_id;
        let topology = state.root.topology().expect("fixture topology");

        let reply = run_cmd(&mut state, register_command(grid_id, 0));

        assert!(matches!(
            reply,
            Reply::Err(SessionError::InvalidSampleRate(0))
        ));
        assert_eq!(state.next_player_id, next_player_id);
        assert_eq!(deck_count(&state), 0);
        assert_eq!(state.root.topology().expect("fixture topology"), topology);
        assert!(state.reserved_session_grid.is_some());
    }

    #[kithara::test]
    fn exhausted_player_identity_preserves_the_canonical_root() {
        let mut state = test_state(start_route_loss_stream);
        let grid_id = attach_player(&mut state);
        let topology = state.root.topology().expect("fixture topology");
        state.next_player_id = u64::MAX;

        let reply = run_cmd(
            &mut state,
            register_command(
                grid_id,
                SessionState::<RouteLossBackend>::DEFAULT_SAMPLE_RATE,
            ),
        );

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
            SessionState::<RouteLossBackend>::DEFAULT_SAMPLE_RATE,
            "until a stream exists the resampler is built for the requested rate"
        );

        let player_id = register_player(&mut state);
        assert!(matches!(
            run_cmd(&mut state, Cmd::QuerySampleRate),
            Reply::SampleRate(SessionSampleRate {
                measured: None,
                requested: SessionState::<RouteLossBackend>::DEFAULT_SAMPLE_RATE,
                ..
            })
        ));
        assert!(matches!(
            run_cmd(
                &mut state,
                Cmd::StartPlayer {
                    player_id,
                    sample_rate: 48_000,
                    master_volume: 1.0,
                },
            ),
            Reply::Ok
        ));
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
    fn explicit_audio_route_invalidation_restarts_stream_without_backend_error() {
        route_loss(RouteLossProbe::reset);

        let mut state = test_state(start_route_loss_stream);
        let player_id = register_player(&mut state);

        assert!(matches!(
            run_cmd(
                &mut state,
                Cmd::StartPlayer {
                    player_id,
                    sample_rate: 0,
                    master_volume: 1.0,
                },
            ),
            Reply::Ok
        ));
        assert!(matches!(
            run_cmd(&mut state, Cmd::QuerySampleRate),
            Reply::SampleRate(SessionSampleRate {
                measured: Some(44_100),
                requested: 0,
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
            run_cmd(
                &mut state,
                Cmd::StartPlayer {
                    player_id,
                    sample_rate: 0,
                    master_volume: 1.0,
                },
            ),
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

        route_loss(|probe| probe.fail_next_poll.store(true, Ordering::SeqCst));
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
    fn failed_stream_restart_is_retried_on_next_tick() {
        route_loss(RouteLossProbe::reset);

        let mut state = test_state(start_route_loss_stream);
        let player_id = register_player(&mut state);

        assert!(matches!(
            run_cmd(
                &mut state,
                Cmd::StartPlayer {
                    player_id,
                    sample_rate: 44_100,
                    master_volume: 1.0,
                },
            ),
            Reply::Ok
        ));
        assert_eq!(
            route_loss(|probe| probe.start_count.load(Ordering::SeqCst)),
            1
        );
        let before_route = host_grid(&state);

        route_loss(|probe| {
            probe.fail_next_poll.store(true, Ordering::SeqCst);
            probe.fail_next_start.store(true, Ordering::SeqCst);
        });
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

    fn start_player_cmd(state: &mut SessionState<RouteLossBackend>, player_id: u64) {
        assert!(matches!(
            run_cmd(
                &mut *state,
                Cmd::StartPlayer {
                    player_id,
                    sample_rate: 44_100,
                    master_volume: 1.0,
                },
            ),
            Reply::Ok
        ));
    }

    fn master_volume_of(state: &SessionState<RouteLossBackend>, player_id: u64) -> f32 {
        state
            .graph
            .decks()
            .find(|player| player.player_id == player_id)
            .expect("player present")
            .master_volume
    }

    #[kithara::test]
    fn batch_master_volume_updates_one_two_and_four_together() {
        route_loss(RouteLossProbe::reset);

        let mut state = test_state(start_route_loss_stream);
        let ids: Vec<u64> = (0..4).map(|_| register_player(&mut state)).collect();
        for &id in &ids {
            start_player_cmd(&mut state, id);
        }

        assert!(matches!(
            run_cmd(
                &mut state,
                Cmd::SetPlayerMasterVolumes {
                    levels: vec![PlayerLevel::new(ids[0], 0.1)],
                },
            ),
            Reply::Ok
        ));
        assert_eq!(master_volume_of(&state, ids[0]), 0.1);

        assert!(matches!(
            run_cmd(
                &mut state,
                Cmd::SetPlayerMasterVolumes {
                    levels: vec![PlayerLevel::new(ids[1], 0.2), PlayerLevel::new(ids[2], 0.3)],
                },
            ),
            Reply::Ok
        ));
        assert_eq!(master_volume_of(&state, ids[1]), 0.2);
        assert_eq!(master_volume_of(&state, ids[2]), 0.3);
        assert_eq!(master_volume_of(&state, ids[3]), 1.0);

        assert!(matches!(
            run_cmd(
                &mut state,
                Cmd::SetPlayerMasterVolumes {
                    levels: vec![
                        PlayerLevel::new(ids[0], 0.4),
                        PlayerLevel::new(ids[1], 0.5),
                        PlayerLevel::new(ids[2], 0.6),
                        PlayerLevel::new(ids[3], 0.7)
                    ],
                },
            ),
            Reply::Ok
        ));
        assert_eq!(master_volume_of(&state, ids[0]), 0.4);
        assert_eq!(master_volume_of(&state, ids[3]), 0.7);
    }

    #[kithara::test]
    fn batch_rejects_duplicate_player_without_mutation() {
        route_loss(RouteLossProbe::reset);

        let mut state = test_state(start_route_loss_stream);
        let id = register_player(&mut state);
        start_player_cmd(&mut state, id);

        assert!(matches!(
            run_cmd(
                &mut state,
                Cmd::SetPlayerMasterVolumes {
                    levels: vec![PlayerLevel::new(id, 0.3), PlayerLevel::new(id, 0.4)],
                },
            ),
            Reply::Err(SessionError::DuplicatePlayer(_))
        ));
        assert_eq!(master_volume_of(&state, id), 1.0);
    }

    #[kithara::test]
    fn batch_rejects_invalid_level_without_mutation() {
        route_loss(RouteLossProbe::reset);

        let mut state = test_state(start_route_loss_stream);
        let a = register_player(&mut state);
        let b = register_player(&mut state);
        start_player_cmd(&mut state, a);
        start_player_cmd(&mut state, b);

        for bad in [f32::NAN, f32::INFINITY, 1.5, -0.1] {
            assert!(matches!(
                run_cmd(
                    &mut state,
                    Cmd::SetPlayerMasterVolumes {
                        levels: vec![PlayerLevel::new(a, 0.5), PlayerLevel::new(b, bad)],
                    },
                ),
                Reply::Err(SessionError::MasterVolumeOutOfRange { .. })
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
    fn batch_rejects_unknown_player_leaving_known_unchanged() {
        route_loss(RouteLossProbe::reset);

        let mut state = test_state(start_route_loss_stream);
        let known = register_player(&mut state);
        start_player_cmd(&mut state, known);
        let unknown = known + 9_999;

        assert!(matches!(
            run_cmd(
                &mut state,
                Cmd::SetPlayerMasterVolumes {
                    levels: vec![PlayerLevel::new(known, 0.2), PlayerLevel::new(unknown, 0.3)],
                },
            ),
            Reply::Err(SessionError::PlayerNotFound(_))
        ));
        assert_eq!(master_volume_of(&state, known), 1.0);
    }

    fn mix_tap_writer(drops: &Arc<AtomicU64>) -> MixTapWriter {
        const TAP_CAPACITY: usize = 1_024;

        let (pcm, _cons) = HeapRb::<f32>::new(TAP_CAPACITY).split();
        MixTapWriter::new(pcm, Arc::clone(drops))
    }

    #[kithara::test]
    fn mix_tap_rejects_a_second_consumer_and_is_cleared_by_idle_teardown() {
        route_loss(RouteLossProbe::reset);

        let mut state = test_state(start_route_loss_stream);
        let id = register_player(&mut state);
        start_player_cmd(&mut state, id);

        let drops = Arc::new(AtomicU64::new(0));
        assert!(matches!(
            run_cmd(
                &mut state,
                Cmd::EnableMixTap {
                    writer: mix_tap_writer(&drops),
                },
            ),
            Reply::Ok
        ));
        assert!(
            matches!(state.mix_tap, Some(MixTap::Installed(_))),
            "a tap armed on a running session reaches the graph at once"
        );

        assert!(
            matches!(
                run_cmd(
                    &mut state,
                    Cmd::EnableMixTap {
                        writer: mix_tap_writer(&drops),
                    },
                ),
                Reply::Err(SessionError::MixTapActive)
            ),
            "a second consumer must be rejected instead of silently replacing the first"
        );

        assert!(matches!(
            run_cmd(&mut state, Cmd::StopPlayer { player_id: id }),
            Reply::Ok
        ));
        assert!(state.session_limiter_node_id.is_none());
        assert!(
            state.mix_tap.is_none(),
            "idle teardown must clear the mix tap with the context it lived in"
        );
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
