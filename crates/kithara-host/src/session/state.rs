use std::num::NonZeroU32;

use arc_swap::ArcSwap;
use firewheel::{
    FirewheelConfig, FirewheelContext,
    channel_config::ChannelCount,
    node::{AudioNode, NodeID},
};
use kithara_bufpool::PoolRegion;
use kithara_command::Live;
use kithara_config::ConfigOwner;
use kithara_events::EventBus;
use kithara_output::OutputGroup;
use kithara_platform::{sync::Arc, time::Duration};
use kithara_play::{DeckMixerConfig, SessionSampleRate, StreamShape};
use kithara_signal::SessionEpoch;
use kithara_warp::{
    BeatGridId, BeatGridRevision, BeatGridSnapshot, BeatGridStamp, MapAxis, SessionAxis,
};
use tracing::{debug, warn};

use super::{
    dispatch::{restart_stream, sample_rate, stream_shape, trace_stream_info},
    graph::tap,
    protocol::{PlayerId, SessionError, StartStreamFn},
    queue::HostProtocol,
    transport::{SessionGridGeneration, TransportControl, install},
};
use crate::{
    api::Tap,
    host::HostSettings,
    rt::{MasterNode, SessionOutput},
};

pub(super) struct Deck<S> {
    pub(super) grid_id: BeatGridId,
    pub(super) bus: EventBus,
    pub(super) player_id: PlayerId,
    pub(super) pools: PoolRegion<S>,
    pub(super) mixer: DeckMixerConfig,
    /// The deck's slot node while the deck runs.
    pub(super) slot_node: Option<NodeID>,
    pub(super) next_slot_id: u64,
}

impl<S> Deck<S> {
    pub(super) fn new(
        player_id: PlayerId,
        grid_id: BeatGridId,
        bus: EventBus,
        pools: PoolRegion<S>,
        mixer: DeckMixerConfig,
    ) -> Self {
        Self {
            bus,
            mixer,
            pools,
            player_id,
            grid_id,
            next_slot_id: 1,
            slot_node: None,
        }
    }

    pub(super) const fn started(&self) -> bool {
        self.slot_node.is_some()
    }
}

#[derive_where::derive_where(Default)]
pub(super) struct GraphRegistry<S> {
    decks: Vec<Deck<S>>,
}

impl<S> GraphRegistry<S> {
    pub(super) fn index_by_grid(&self, grid_id: BeatGridId) -> Option<usize> {
        self.decks
            .iter()
            .position(|candidate| candidate.grid_id == grid_id)
    }

    pub(super) fn index_by_player(&self, player_id: PlayerId) -> Option<usize> {
        self.decks
            .iter()
            .position(|candidate| candidate.player_id == player_id)
    }

    pub(super) fn insert(&mut self, deck: Deck<S>) -> Result<(), SessionError> {
        if self
            .decks
            .iter()
            .any(|candidate| candidate.grid_id == deck.grid_id)
        {
            return Err(SessionError::Graph(
                "player grid is already projected into the session graph".to_owned(),
            ));
        }
        self.decks.push(deck);
        Ok(())
    }

    pub(super) fn remove(&mut self, index: usize) -> Option<Deck<S>> {
        (index < self.decks.len()).then(|| self.decks.remove(index))
    }

    delegate::delegate! {
        to self.decks {
            #[call(get)]
            pub(super) fn deck(&self, index: usize) -> Option<&Deck<S>>;
            #[call(get_mut)]
            pub(super) fn deck_mut(&mut self, index: usize) -> Option<&mut Deck<S>>;
            #[call(iter)]
            pub(super) fn decks(&self) -> impl Iterator<Item = &Deck<S>>;
            pub(super) fn len(&self) -> usize;
        }
    }
}

pub(super) enum TapSlot {
    Requested(OutputGroup),
    Installed(NodeID),
}

#[derive(Default)]
pub(super) struct Taps {
    master: Option<TapSlot>,
    output: Option<TapSlot>,
}

impl Taps {
    pub(super) fn slot(&mut self, tap: Tap) -> &mut Option<TapSlot> {
        match tap {
            Tap::Master => &mut self.master,
            Tap::Output => &mut self.output,
        }
    }
}

/// The Host's session grid, which its transport publishes into, and the
/// decks attached to the session.
#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub(crate) struct HostRoot {
    /// The session grid the transport last committed.
    #[field(get, vis = "pub(crate)")]
    grid: BeatGridSnapshot,
    decks: Vec<BeatGridId>,
}

impl HostRoot {
    /// A root `id` with no deck, its grid not yet live on a session axis at
    /// `sample_rate`.
    pub(crate) fn new(id: BeatGridId, sample_rate: NonZeroU32) -> Self {
        Self {
            grid: BeatGridSnapshot::unavailable(
                id,
                BeatGridRevision::first(),
                MapAxis::Session(SessionAxis::new(sample_rate, SessionEpoch::new(0))),
            ),
            decks: Vec::new(),
        }
    }

    pub(crate) fn id(&self) -> BeatGridId {
        self.grid.id()
    }

    /// Takes the session grid the transport committed.
    pub(super) fn publish(&mut self, grid: BeatGridSnapshot) {
        self.grid = grid;
    }

    /// Takes the grid of a route boundary: a later revision `stamp` names,
    /// on the session axis of `epoch` at `sample_rate`, with no geometry
    /// until the transport commits one.
    pub(super) fn publish_unavailable(
        &mut self,
        stamp: BeatGridStamp,
        sample_rate: NonZeroU32,
        epoch: SessionEpoch,
    ) {
        self.publish(BeatGridSnapshot::unavailable(
            stamp.grid_id(),
            stamp.revision(),
            MapAxis::Session(SessionAxis::new(sample_rate, epoch)),
        ));
    }

    /// Whether the deck `grid_id` is attached.
    pub(super) fn holds(&self, grid_id: BeatGridId) -> bool {
        self.decks.contains(&grid_id)
    }

    /// Adds one deck.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::DeckAttached`] when its identity is already
    /// in the session.
    pub(crate) fn attach(&mut self, grid_id: BeatGridId) -> Result<(), SessionError> {
        if self.holds(grid_id) {
            return Err(SessionError::DeckAttached(grid_id));
        }
        self.decks.push(grid_id);
        Ok(())
    }

    /// Removes the deck `grid_id`.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::DeckNotFound`] when no deck has that identity.
    pub(crate) fn detach(&mut self, grid_id: BeatGridId) -> Result<(), SessionError> {
        let index = self
            .decks
            .iter()
            .position(|held| *held == grid_id)
            .ok_or(SessionError::DeckNotFound(grid_id))?;
        self.decks.remove(index);
        Ok(())
    }

    fn decks(&self) -> Box<[BeatGridId]> {
        self.decks.as_slice().into()
    }
}

struct RootSnapshot {
    decks: Box<[BeatGridId]>,
    grid: BeatGridSnapshot,
    settings: HostSettings,
    stream_shape: Option<StreamShape>,
    sample_rate: SessionSampleRate,
}

#[derive(Clone)]
pub(crate) struct RootView(Arc<ArcSwap<RootSnapshot>>);

impl RootView {
    pub(crate) fn new(root: &HostRoot, settings: HostSettings) -> Self {
        Self(Arc::new(ArcSwap::from_pointee(RootSnapshot {
            settings,
            decks: root.decks(),
            grid: root.grid.clone(),
            stream_shape: None,
            sample_rate: SessionSampleRate::new(None, settings.sample_rate().get()),
        })))
    }

    fn publish(
        &self,
        root: &HostRoot,
        settings: HostSettings,
        stream_shape: Option<StreamShape>,
        sample_rate: SessionSampleRate,
    ) {
        self.0.store(Arc::new(RootSnapshot {
            settings,
            stream_shape,
            sample_rate,
            decks: root.decks(),
            grid: root.grid.clone(),
        }));
    }

    /// Whether the deck `grid_id` is in the session.
    pub(crate) fn holds(&self, grid_id: BeatGridId) -> bool {
        self.0.load().decks.contains(&grid_id)
    }

    /// Whether the session holds no deck.
    pub(crate) fn is_empty(&self) -> bool {
        self.0.load().decks.is_empty()
    }

    delegate::delegate! {
        to self.0 {
            #[call(load)]
            #[expr($.grid.clone())]
            pub(crate) fn grid(&self) -> BeatGridSnapshot;
            #[call(load)]
            #[expr($.sample_rate)]
            pub(crate) fn sample_rate(&self) -> SessionSampleRate;
            #[call(load)]
            #[expr($.settings)]
            pub(crate) fn settings(&self) -> HostSettings;
            #[call(load)]
            #[expr($.stream_shape)]
            pub(crate) fn stream_shape(&self) -> Option<StreamShape>;
        }
    }
}

pub(crate) struct SessionState<T, S> {
    pub(super) graph: GraphRegistry<S>,
    pub(super) root: HostRoot,
    pub(super) output: SessionOutput,
    pub(super) session_metronome_node_id: Option<NodeID>,
    pub(super) ctx: Option<FirewheelContext>,
    pub(super) taps: Taps,
    /// The pause/resume fade length the session asks Firewheel for, in frames.
    /// `None` leaves Firewheel's own default in place.
    pub(super) requested_declick_frames: Option<NonZeroU32>,
    pub(super) requested_max_block_frames: Option<NonZeroU32>,
    pub(super) reserved_session_grid: Option<SessionGridGeneration>,
    pub(super) session_limiter_node_id: Option<NodeID>,
    pub(super) session_output_node_id: Option<NodeID>,
    pub(super) stream: Option<T>,
    pub(super) transport_control: Option<TransportControl>,
    pub(super) next_player_id: PlayerId,
    pub(super) root_view: RootView,
    /// The Host settings as the render graph confirmed them, with the
    /// changes still on their way to it.
    pub(super) settings: Live<HostSettings, HostProtocol>,
    pub(super) start_stream_fn: StartStreamFn<T>,
    /// Set when the output device is acquired once and cannot be rebuilt, so
    /// an idle session must keep it rather than release it.
    pub(super) retains_output: bool,
    pub(super) stream_needs_restart: bool,
}

/// The stream outlives nothing: it is dropped before the context.
///
/// Firewheel hands the stream its processor and waits, on its own drop, for
/// that processor to come back. Declaration order would drop the context
/// first, leaving it to wait out its whole deactivation timeout for a
/// processor this state still owns.
impl<T, S> Drop for SessionState<T, S> {
    fn drop(&mut self) {
        self.stream.take();
        self.ctx.take();
    }
}

impl<T, S> SessionState<T, S> {
    #[cfg(test)]
    pub(crate) const DEFAULT_SAMPLE_RATE: u32 = 44_100;

    /// Creates session state with its own musical-grid topology, asking for
    /// the output at the sample rate its settings name.
    #[must_use]
    pub(crate) fn new<F>(
        root: HostRoot,
        root_view: RootView,
        requested_max_block_frames: Option<NonZeroU32>,
        requested_declick_frames: Option<NonZeroU32>,
        output: SessionOutput,
        settings: Live<HostSettings, HostProtocol>,
        start_stream_fn: F,
    ) -> Self
    where
        F: FnMut(&mut FirewheelContext, u32) -> Result<T, String> + Send + 'static,
    {
        let grid_id = root.id();
        let mut generation = SessionGridGeneration::new(grid_id);
        generation.commit_revision(BeatGridRevision::first());
        let state = Self {
            settings,
            requested_max_block_frames,
            requested_declick_frames,
            output,
            session_metronome_node_id: None,
            root,
            root_view,
            start_stream_fn: Box::new(start_stream_fn),
            ctx: None,
            stream: None,
            transport_control: None,
            taps: Taps::default(),
            next_player_id: 1,
            session_output_node_id: None,
            session_limiter_node_id: None,
            retains_output: false,
            stream_needs_restart: false,
            reserved_session_grid: Some(generation),
            graph: GraphRegistry::default(),
        };
        state.publish_root();
        state
    }

    pub(super) fn publish_root(&self) {
        self.root_view.publish(
            &self.root,
            *self.settings.config(),
            stream_shape(self),
            sample_rate(self),
        );
    }
}

/// Adds a node to the graph, turning the rejection Firewheel now reports into
/// the session's own graph error. A node the graph refuses is a wiring bug, not
/// a runtime condition the session can route around.
pub(super) fn add_graph_node<N: AudioNode + 'static>(
    ctx: &mut FirewheelContext,
    node: N,
) -> Result<NodeID, SessionError> {
    ctx.add_node(node, None)
        .map_err(|err| SessionError::Graph(format!("audio graph rejected a node: {err}")))
}

pub(super) fn register_player<T, S>(
    state: &mut SessionState<T, S>,
    grid_id: BeatGridId,
    bus: EventBus,
    pools: PoolRegion<S>,
    mixer: DeckMixerConfig,
) -> Result<PlayerId, SessionError> {
    let player_id = state.next_player_id;
    let next_player_id = player_id
        .checked_add(1)
        .ok_or(SessionError::PlayerIdExhausted)?;
    if !state.root.holds(grid_id) {
        return Err(SessionError::DeckNotFound(grid_id));
    }
    state
        .graph
        .insert(Deck::new(player_id, grid_id, bus, pools, mixer))?;
    state.next_player_id = next_player_id;
    debug!(
        player_id,
        players = state.graph.len(),
        "[KITHARA-ROUTE] session player registered"
    );
    Ok(player_id)
}

pub(super) fn ensure_ctx<T, S>(state: &mut SessionState<T, S>) -> Result<(), SessionError> {
    ensure_stream_ready(state)?;
    ensure_session_output(state)
}

fn ensure_stream_ready<T, S>(state: &mut SessionState<T, S>) -> Result<(), SessionError> {
    if state.ctx.is_none() {
        return create_firewheel_context(state);
    }

    if state.stream_needs_restart {
        debug!("[KITHARA-ROUTE] ensuring stopped stream is restarted");
        restart_stream(state)?;
    }

    Ok(())
}

/// Converts the fade through `Duration` rather than casting directly, since Firewheel takes the
/// fade in seconds while the frame count is the session's own unit.
fn create_firewheel_context<T, S>(state: &mut SessionState<T, S>) -> Result<(), SessionError> {
    let sample_rate = state.settings.config().sample_rate().get();
    debug!(sample_rate, "[KITHARA-ROUTE] creating firewheel context");
    let mut config = FirewheelConfig {
        num_graph_outputs: ChannelCount::STEREO,
        ..FirewheelConfig::default()
    };
    if let Some(declick_frames) = state.requested_declick_frames {
        config.declick_seconds =
            Duration::from_secs_f64(f64::from(declick_frames.get()) / f64::from(sample_rate))
                .as_secs_f32();
    }
    let mut ctx = FirewheelContext::new(config);
    let session_grid = state
        .reserved_session_grid
        .take()
        .ok_or_else(|| SessionError::Graph("session grid generation is missing".to_owned()))?;
    let transport_control = match install(&mut ctx, session_grid, *state.settings.config()) {
        Ok(control) => control,
        Err(error) => {
            state.reserved_session_grid = Some(session_grid);
            return Err(SessionError::Graph(error.into()));
        }
    };
    let stream = match (state.start_stream_fn)(&mut ctx, sample_rate) {
        Ok(stream) => stream,
        Err(error) => {
            state.reserved_session_grid = Some(session_grid);
            return Err(SessionError::StreamStart(error));
        }
    };
    state.ctx = Some(ctx);
    state.stream = Some(stream);
    state.transport_control = Some(transport_control);
    state.stream_needs_restart = false;
    state.publish_root();
    trace_stream_info(state, "start-stream");
    debug!(sample_rate, "[KITHARA-ROUTE] firewheel context ready");
    Ok(())
}

fn ensure_session_output<T, S>(state: &mut SessionState<T, S>) -> Result<(), SessionError> {
    if state.session_output_node_id.is_none() {
        return create_session_output(state);
    }

    Ok(())
}

fn create_session_output<T, S>(state: &mut SessionState<T, S>) -> Result<(), SessionError> {
    debug!("[KITHARA-ROUTE] creating session output graph");
    let limiter = state.output.limiter();
    let metronome = state.output.metronome(state.settings.config().metronome());
    let Some(ref mut fw_ctx) = state.ctx else {
        return Err(SessionError::NoContext);
    };
    let session_id = add_graph_node(fw_ctx, MasterNode)?;
    let limiter_id = add_graph_node(fw_ctx, limiter)?;
    let metronome_id = add_graph_node(fw_ctx, metronome)?;
    let graph_out = fw_ctx.graph_out_node_id();
    fw_ctx
        .connect(session_id, limiter_id, &[(0, 0), (1, 1)], false)
        .map_err(|err| {
            SessionError::Graph(format!("connect session output to limiter failed: {err}"))
        })?;
    fw_ctx
        .connect(limiter_id, metronome_id, &[(0, 0), (1, 1)], false)
        .map_err(|err| {
            SessionError::Graph(format!("connect limiter to metronome failed: {err}"))
        })?;
    fw_ctx
        .connect(metronome_id, graph_out, &[(0, 0), (1, 1)], false)
        .map_err(|err| {
            SessionError::Graph(format!("connect metronome to graph_out failed: {err}"))
        })?;
    if let Err(err) = fw_ctx.update() {
        warn!("session graph update after output init failed: {err:?}");
    }
    state.session_output_node_id = Some(session_id);
    state.session_limiter_node_id = Some(limiter_id);
    state.session_metronome_node_id = Some(metronome_id);
    tap::install_requested(state)?;
    debug!(
        ?session_id,
        ?limiter_id,
        ?metronome_id,
        "[KITHARA-ROUTE] session output graph ready"
    );
    Ok(())
}
