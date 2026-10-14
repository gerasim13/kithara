use std::num::NonZeroUsize;

use firewheel::{
    FirewheelContext, Volume, diff::Memo, dsp::volume::amp_to_linear_volume_clamped, node::NodeID,
    nodes::volume::VolumeNode,
};
use kithara_bufpool::HasPool;
use kithara_effects::{
    GainDb,
    eq::{EqBandConfig, EqConfig},
};
use kithara_output::OutputGroup;
use kithara_signal::FaderValue;
use kithara_warp::{BeatGrid, MapAxis};
use tracing::{debug, warn};

use super::{
    protocol::{AllocatedSlot, PlayerId, PlayerLevel, Reply, SessionError},
    queue::settle_receipts,
    state::{
        Deck, GraphRegistry, SessionState, SlotNodes, TapSlot, Taps, add_graph_node, ensure_ctx,
        prepare_eq_layout,
    },
};
use crate::{
    api::SlotId,
    bridge::slot_channels,
    rt::{MasterEqNode, PlayerNode, TapNode},
};
/// A level is a linear amplitude, but `Volume::Linear` is a fader taper that
/// squares its argument, so it must be converted rather than passed through.
pub(super) fn master_gain(level: f32) -> Volume {
    Volume::Linear(amp_to_linear_volume_clamped(level, 0.0))
}
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

pub(super) fn deck_at<T, S>(
    state: &SessionState<T, S>,
    index: usize,
) -> Result<&Deck<S>, SessionError> {
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
        master_volume: f32,
        render_quantum_frames: Option<NonZeroUsize>,
        response_budget_frames: Option<NonZeroUsize>,
    ) -> Result<(), SessionError>
    where
        S: HasPool<f32> + Send + Sync + 'static,
    {
        debug!(player_id, master_volume, "[KITHARA-ROUTE] starting player");
        ensure_ctx(state)?;
        validate_response_geometry(state, render_quantum_frames, response_budget_frames)?;
        let idx = player_index(state, player_id)?;
        let Some(session_output_id) = state.session_output_node_id else {
            return Err(graph_state("session output node is not initialised"));
        };
        let (ctx, graph) = (&mut state.ctx, &mut state.graph);
        let fw_ctx = ctx.as_mut().ok_or(SessionError::NoContext)?;
        let player = deck_at_mut(graph, idx)?;
        if player.started {
            return Err(SessionError::AlreadyStarted(player_id));
        }
        let eq_config = EqConfig::builder(player.pools.clone()).build();
        let mut master_eq = MasterEqNode::new(eq_config, &player.eq_layout);
        for (band, gain) in player.shared_eq.snapshot().into_iter().enumerate() {
            master_eq.set_gain(band, GainDb::from(gain));
        }
        let master_eq_memo = Memo::new(master_eq.clone());
        let master_eq_id = add_graph_node(fw_ctx, master_eq)?;
        let master_volume = VolumeNode {
            volume: master_gain(player.master_volume),
            ..VolumeNode::default()
        };
        let master_volume_memo = Memo::new(master_volume);
        let master_volume_id = add_graph_node(fw_ctx, master_volume)?;
        let eq_to_volume = "connect player master_eq->master_vol";
        connect_stereo(fw_ctx, master_eq_id, master_volume_id, eq_to_volume)?;
        let volume_to_output = "connect player master_vol->session_output";
        connect_stereo(
            fw_ctx,
            master_volume_id,
            session_output_id,
            volume_to_output,
        )?;
        if let Err(err) = fw_ctx.update() {
            warn!(player_id, "graph update after player start failed: {err:?}");
        }
        player.master_eq_node_id = Some(master_eq_id);
        player.master_eq_memo = Some(master_eq_memo);
        player.master_volume_node_id = Some(master_volume_id);
        player.master_volume_memo = Some(master_volume_memo);
        player.started = true;
        debug!(
            player_id,
            ?master_eq_id,
            ?master_volume_id,
            "[KITHARA-ROUTE] player graph started"
        );
        Ok(())
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
            if !player.started {
                return Err(SessionError::NotRunning(player.player_id));
            }
            if let Some(fw_ctx) = ctx {
                remove_player_graph(fw_ctx, player);
                if let Err(err) = fw_ctx.update() {
                    warn!(
                        player_id = player.player_id,
                        "graph update after player stop failed: {err:?}"
                    );
                }
            } else {
                clear_player_graph_state(player);
            }
            player.started = false;
        }
        shutdown_if_idle(state)?;
        debug!("[KITHARA-ROUTE] player stopped");
        Ok(())
    }
    /// Release the output device once no player is left to feed it. A media
    /// app that has stopped playing must not keep the platform's output
    /// engaged; the next `start_player` builds a fresh context.
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
        let idle = state.graph.decks().all(|deck| !deck.started);
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
            let MapAxis::Session(axis) = state.root.snapshot().axis() else {
                return Err(graph_state(
                    "session host published a non-session grid during idle shutdown",
                ));
            };
            let sample_rate = axis.sample_rate();
            state.root.publish_unavailable_grid(
                session_stamp,
                sample_rate,
                session_grid_generation.epoch(),
            )?;
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
    pub(super) fn remove_player_graph<S>(fw_ctx: &mut FirewheelContext, player: &mut Deck<S>) {
        let player_id = player.player_id;
        for slot in player.slots.drain(..) {
            if let Err(err) = fw_ctx.remove_node(slot.volume_node_id) {
                warn!(player_id, ?err, "failed to remove slot volume node");
            }
            if let Err(err) = fw_ctx.remove_node(slot.player_node_id) {
                warn!(player_id, ?err, "failed to remove slot player node");
            }
        }
        if let Some(master_id) = player.master_volume_node_id.take()
            && let Err(err) = fw_ctx.remove_node(master_id)
        {
            warn!(player_id, ?err, "failed to remove player master vol node");
        }
        if let Some(master_eq_id) = player.master_eq_node_id.take()
            && let Err(err) = fw_ctx.remove_node(master_eq_id)
        {
            warn!(player_id, ?err, "failed to remove player master eq node");
        }
        clear_player_graph_state(player);
    }
    pub(super) fn clear_player_graph_state<S>(player: &mut Deck<S>) {
        player.master_eq_memo = None;
        player.master_volume_memo = None;
    }
}

pub(super) mod slots {
    use super::*;

    pub(in crate::session) fn allocate_slot<T, S>(
        state: &mut SessionState<T, S>,
        player_id: PlayerId,
    ) -> Result<Reply, SessionError>
    where
        S: HasPool<f32> + Send + Sync + 'static,
    {
        debug!(player_id, "[KITHARA-ROUTE] allocating player slot");
        let idx = player_index(state, player_id)?;
        if !deck_at(state, idx)?.started {
            return Err(SessionError::NotRunning(player_id));
        }
        let master_eq_id = deck_at(state, idx)?.master_eq_node_id;
        let (fw_ctx, master_eq_id) = match (&mut state.ctx, master_eq_id) {
            (None, _) => return Err(SessionError::NoContext),
            (Some(_), None) => return Err(graph_state("player master eq node is not initialised")),
            (Some(fw_ctx), Some(master_eq_id)) => (fw_ctx, master_eq_id),
        };
        let player = deck_at_mut(&mut state.graph, idx)?;
        let slot_id = SlotId::new(player.next_slot_id);
        player.next_slot_id += 1;
        let shared_eq = player.shared_eq.clone();
        let (inputs, control) = slot_channels(shared_eq);
        let player_node = PlayerNode::new(inputs, player.pools.clone(), player.gate_smoothing)
            .with_session_context();
        let player_node_id = add_graph_node(fw_ctx, player_node)?;
        let slot_volume = VolumeNode::from_linear(1.0);
        let slot_volume_memo = Memo::new(slot_volume);
        let slot_volume_id = add_graph_node(fw_ctx, slot_volume)?;
        let player_to_slot = "connect player->slot_volume";
        connect_stereo(fw_ctx, player_node_id, slot_volume_id, player_to_slot)?;
        let slot_to_master = "connect slot_volume->player_master_eq";
        connect_stereo(fw_ctx, slot_volume_id, master_eq_id, slot_to_master)?;
        if let Err(err) = fw_ctx.update() {
            warn!(
                player_id,
                ?slot_id,
                "graph update after slot allocate failed: {err:?}"
            );
        }
        player.slots.push(SlotNodes {
            slot_id,
            player_node_id,
            volume_memo: slot_volume_memo,
            volume_node_id: slot_volume_id,
        });
        debug!(
            player_id,
            ?slot_id,
            ?player_node_id,
            ?slot_volume_id,
            slots = player.slots.len(),
            "[KITHARA-ROUTE] player slot allocated"
        );
        let reply = Reply::SlotAllocated(AllocatedSlot::new(control, slot_id));
        Ok(reply)
    }
    pub(in crate::session) fn release_slot<T, S>(
        state: &mut SessionState<T, S>,
        player_id: PlayerId,
        slot: SlotId,
    ) -> Result<(), SessionError> {
        debug!(player_id, ?slot, "[KITHARA-ROUTE] releasing player slot");
        let idx = player_index(state, player_id)?;
        let slot_nodes = {
            let player = deck_at_mut(&mut state.graph, idx)?;
            if !player.started {
                return Err(SessionError::NotRunning(player_id));
            }
            take_slot(player, slot)?
        };
        let fw_ctx = state.ctx.as_mut().ok_or(SessionError::NoContext)?;
        remove_slot_graph(fw_ctx, player_id, &slot_nodes);
        debug!(
            player_id,
            ?slot_nodes,
            "[KITHARA-ROUTE] player slot released"
        );
        Ok(())
    }
    pub(super) fn take_slot<S>(
        player: &mut Deck<S>,
        slot: SlotId,
    ) -> Result<SlotNodes, SessionError> {
        let Some(slot_idx) = player.slots.iter().position(|s| s.slot_id == slot) else {
            return Err(SessionError::SlotNotFound(slot));
        };
        Ok(player.slots.remove(slot_idx))
    }
    pub(super) fn remove_slot_graph(
        fw_ctx: &mut FirewheelContext,
        player_id: PlayerId,
        slot: &SlotNodes,
    ) {
        if let Err(err) = fw_ctx.remove_node(slot.volume_node_id) {
            warn!(player_id, ?err, "failed to remove slot volume node");
        }
        if let Err(err) = fw_ctx.remove_node(slot.player_node_id) {
            warn!(player_id, ?err, "failed to remove slot player node");
        }
        if let Err(err) = fw_ctx.update() {
            warn!(player_id, "graph update after slot release failed: {err:?}");
        }
    }
}

pub(super) mod controls {
    use super::*;

    /// Validates the whole request before mutating anything, so an invalid
    /// entry leaves the batch untouched. Omitted players are unchanged.
    pub(in crate::session) fn set_player_master_volumes<T, S>(
        state: &mut SessionState<T, S>,
        levels: &[PlayerLevel],
    ) -> Result<(), SessionError> {
        let mut resolved: Vec<(usize, f32)> = Vec::with_capacity(levels.len());
        for &PlayerLevel {
            player_id, level, ..
        } in levels
        {
            if !level.is_finite() || !(0.0..=1.0).contains(&level) {
                return Err(SessionError::MasterVolumeOutOfRange { player_id, level });
            }
            let idx = player_index(state, player_id)?;
            if resolved.iter().any(|&(seen, _)| seen == idx) {
                return Err(SessionError::DuplicatePlayer(player_id));
            }
            let player = deck_at(state, idx)?;
            if player.started
                && (state.ctx.is_none()
                    || player.master_volume_node_id.is_none()
                    || player.master_volume_memo.is_none())
            {
                return Err(graph_state("player master vol graph is not initialised"));
            }
            resolved.push((idx, level));
        }
        for &(idx, level) in &resolved {
            apply_master_volume(state, idx, level)?;
        }
        Ok(())
    }

    fn apply_master_volume<T, S>(
        state: &mut SessionState<T, S>,
        idx: usize,
        volume: f32,
    ) -> Result<(), SessionError> {
        let (ctx, graph) = (&mut state.ctx, &mut state.graph);
        let player = deck_at_mut(graph, idx)?;
        player.master_volume = volume;
        if let (Some(fw_ctx), Some(master_id), Some(memo)) = (
            ctx,
            player.master_volume_node_id,
            &mut player.master_volume_memo,
        ) {
            memo.volume = master_gain(volume);
            let mut queue = fw_ctx.event_queue(master_id);
            memo.update_memo(&mut queue);
        }
        Ok(())
    }
    pub(in crate::session) fn set_player_slot_volume<T, S>(
        state: &mut SessionState<T, S>,
        player_id: PlayerId,
        slot: SlotId,
        volume: FaderValue,
    ) -> Result<(), SessionError> {
        let idx = player_index(state, player_id)?;
        if !deck_at(state, idx)?.started {
            return Err(SessionError::NotRunning(player_id));
        }
        let (ctx, graph) = (&mut state.ctx, &mut state.graph);
        let Some(slot_nodes) = deck_at_mut(graph, idx)?
            .slots
            .iter_mut()
            .find(|s| s.slot_id == slot)
        else {
            return Err(SessionError::SlotNotFound(slot));
        };
        let fw_ctx = ctx.as_mut().ok_or(SessionError::NoContext)?;
        slot_nodes.volume_memo.volume = Volume::Linear(volume.into());
        let mut queue = fw_ctx.event_queue(slot_nodes.volume_node_id);
        slot_nodes.volume_memo.update_memo(&mut queue);
        Ok(())
    }
    pub(in crate::session) fn set_player_eq_gain<T, S>(
        state: &mut SessionState<T, S>,
        player_id: PlayerId,
        band: usize,
        gain_db: f32,
    ) -> Result<(), SessionError> {
        let idx = player_index(state, player_id)?;
        let player = deck_at(state, idx)?;
        if band >= player.eq_layout.len() {
            return Err(SessionError::EqBandOutOfRange {
                band,
                bands: player.eq_layout.len(),
            });
        }
        player
            .shared_eq
            .set_gain(band, GainDb::from(gain_db))
            .map_err(|_| SessionError::EqBandOutOfRange {
                band,
                bands: player.eq_layout.len(),
            })?;
        if !player.started {
            return Ok(());
        }
        let (ctx, graph) = (&mut state.ctx, &mut state.graph);
        let player = deck_at_mut(graph, idx)?;
        let fw_ctx = ctx.as_mut().ok_or(SessionError::NoContext)?;
        let Some(master_eq_id) = player.master_eq_node_id else {
            return Err(graph_state("player master eq node is not initialised"));
        };
        let Some(memo) = &mut player.master_eq_memo else {
            return Err(graph_state("player master eq memo is not initialised"));
        };
        if band >= memo.band_count() {
            return Err(SessionError::EqBandOutOfRange {
                band,
                bands: memo.band_count(),
            });
        }
        memo.set_gain(band, GainDb::from(gain_db));
        let mut queue = fw_ctx.event_queue(master_eq_id);
        memo.update_memo(&mut queue);
        Ok(())
    }
    pub(in crate::session) fn set_player_eq_layout<T, S>(
        state: &mut SessionState<T, S>,
        player_id: PlayerId,
        eq_layout: Vec<EqBandConfig>,
    ) -> Result<(), SessionError>
    where
        S: HasPool<f32> + Send + Sync + 'static,
    {
        let idx = player_index(state, player_id)?;
        let (eq_layout, gains) = prepare_eq_layout(eq_layout);
        if !deck_at(state, idx)?.started {
            let player = deck_at_mut(&mut state.graph, idx)?;
            player.eq_layout = eq_layout;
            player.shared_eq.replace(&gains);
            return Ok(());
        }

        let (master_eq_id, pools) = {
            let player = deck_at(state, idx)?;
            let master_eq_id = player
                .master_eq_node_id
                .ok_or_else(|| graph_state("player master eq node is not initialised"))?;
            (master_eq_id, player.pools.clone())
        };
        let fw_ctx = state.ctx.as_mut().ok_or(SessionError::NoContext)?;
        let sample_rate = fw_ctx
            .stream_info()
            .map(|info| info.sample_rate)
            .ok_or_else(|| graph_state("session stream is not running"))?;
        let master_eq = MasterEqNode::new(EqConfig::builder(pools).build(), &eq_layout);
        let event = master_eq.layout_event(sample_rate).map_err(|error| {
            SessionError::Graph(format!("prepare master EQ layout failed: {error}"))
        })?;
        fw_ctx.queue_event_for(master_eq_id, event);

        let player = deck_at_mut(&mut state.graph, idx)?;
        player.eq_layout = eq_layout;
        player.shared_eq.replace(&gains);
        player.master_eq_memo = Some(Memo::new(master_eq));
        Ok(())
    }
}
