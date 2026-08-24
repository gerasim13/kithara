use std::num::NonZeroU32;

use firewheel::{
    FirewheelConfig, FirewheelCtx, backend::AudioBackend, channel_config::ChannelCount, diff::Memo,
    node::NodeID, nodes::volume::VolumeNode,
};
use kithara_audio::{
    BeatMapId, BeatMapRevision, BeatMapSnapshot, EqBandConfig, HostEpoch, SyncAdmission,
    SyncCapability, SyncError, SyncGroup, SyncMember, SyncOperation, SyncOperationId,
    TopologyOperation, TopologyRevision,
};
use kithara_bufpool::PcmPool;
use kithara_events::EventBus;
use tracing::{debug, warn};

use super::{
    dispatch::{restart_stream, trace_stream_info},
    graph::{ducking_gain, tap},
    protocol::{PlayerId, SessionError, StartStreamFn},
    sync::{Host, unavailable_map},
    transport::{HostMapGeneration, SessionTransportState, TransportControl, install},
};
use crate::{
    api::{SessionDuckingMode, SlotId},
    bridge::{MixTapWriter, SharedEq},
    resource::AssetMapRegistry,
    rt::{LimiterNode, MasterEqNode},
};

#[derive(Debug)]
pub(super) struct SlotNodes {
    pub(super) volume_memo: Memo<VolumeNode>,
    pub(super) player_node_id: NodeID,
    pub(super) volume_node_id: NodeID,
    pub(super) slot_id: SlotId,
}

pub(super) struct Deck {
    pub(super) bus: EventBus,
    pub(super) master_eq_memo: Option<Memo<MasterEqNode>>,
    pub(super) master_eq_node_id: Option<NodeID>,
    pub(super) master_volume_memo: Option<Memo<VolumeNode>>,
    pub(super) master_volume_node_id: Option<NodeID>,
    pub(super) pcm_pool: PcmPool,
    pub(super) player_id: PlayerId,
    pub(super) map: BeatMapSnapshot,
    pub(super) tracks: Vec<SyncMember<Self>>,
    pub(super) next_operation: Option<SyncOperationId>,
    pub(super) topology_revision: TopologyRevision,
    pub(super) unavailable: Option<(SyncOperationId, SyncCapability)>,
    pub(super) shared_eq: SharedEq,
    pub(super) eq_layout: Vec<EqBandConfig>,
    pub(super) slots: Vec<SlotNodes>,
    pub(super) started: bool,
    pub(super) master_volume: f32,
    pub(super) next_slot_id: u64,
}

impl Deck {
    pub(super) fn new(
        player_id: PlayerId,
        id: BeatMapId,
        bus: EventBus,
        eq_layout: Vec<EqBandConfig>,
        pcm_pool: PcmPool,
        sample_rate: NonZeroU32,
    ) -> Self {
        let (eq_layout, gains) = prepare_eq_layout(eq_layout);
        let band_count = eq_layout.len();
        let shared_eq = SharedEq::new(band_count);
        shared_eq.replace(&gains);
        Self {
            bus,
            eq_layout,
            pcm_pool,
            player_id,
            map: unavailable_map(id, BeatMapRevision::first(), sample_rate, HostEpoch::new(0)),
            tracks: Vec::new(),
            next_operation: Some(SyncOperationId::first()),
            topology_revision: TopologyRevision::first(),
            unavailable: None,
            master_eq_memo: None,
            master_eq_node_id: None,
            master_volume: 1.0,
            master_volume_memo: None,
            master_volume_node_id: None,
            next_slot_id: 1,
            shared_eq,
            slots: Vec::new(),
            started: false,
        }
    }
}

pub(super) fn prepare_eq_layout(mut eq_layout: Vec<EqBandConfig>) -> (Vec<EqBandConfig>, Vec<f32>) {
    for band in &mut eq_layout {
        band.set_gain_db(band.gain_db());
    }
    let gains = eq_layout.iter().map(EqBandConfig::gain_db).collect();
    (eq_layout, gains)
}

pub(super) enum MixTap {
    Requested(MixTapWriter),
    Installed(NodeID),
}

pub struct SessionState<B: AudioBackend> {
    pub(super) ctx: Option<FirewheelCtx<B>>,
    pub(super) transport_control: Option<TransportControl>,
    pub(super) mix_tap: Option<MixTap>,
    pub(super) session_limiter_node_id: Option<NodeID>,
    pub(super) session_output_memo: Option<Memo<VolumeNode>>,
    pub(super) session_output_node_id: Option<NodeID>,
    pub(super) next_player_id: PlayerId,
    pub(super) session_ducking: SessionDuckingMode,
    pub(super) start_stream_fn: StartStreamFn<B>,
    pub(super) stream_needs_restart: bool,
    pub(super) sample_rate_hint: u32,
    pub(super) transport: SessionTransportState,
    beat_maps: AssetMapRegistry,
    pub(super) host_map_generation: Option<HostMapGeneration>,
    pub(super) host: Option<Host>,
}

impl<B: AudioBackend> SessionState<B> {
    pub const DEFAULT_SAMPLE_RATE: u32 = 44_100;

    /// Creates session state with its own musical-map namespace.
    #[must_use]
    pub fn new<F>(start_stream_fn: F) -> Self
    where
        F: FnMut(&mut FirewheelCtx<B>, u32) -> Result<(), String> + Send + 'static,
    {
        Self {
            start_stream_fn: Box::new(start_stream_fn),
            ctx: None,
            transport_control: None,
            mix_tap: None,
            next_player_id: 1,
            sample_rate_hint: Self::DEFAULT_SAMPLE_RATE,
            session_ducking: SessionDuckingMode::Off,
            session_output_memo: None,
            session_output_node_id: None,
            session_limiter_node_id: None,
            stream_needs_restart: false,
            transport: SessionTransportState::default(),
            beat_maps: AssetMapRegistry::default(),
            host_map_generation: None,
            host: None,
        }
    }

    pub const fn ctx_mut(&mut self) -> Option<&mut FirewheelCtx<B>> {
        self.ctx.as_mut()
    }

    pub(crate) fn beat_maps(&self) -> AssetMapRegistry {
        self.beat_maps.clone()
    }
}

pub(super) fn register_player<B: AudioBackend>(
    state: &mut SessionState<B>,
    bus: EventBus,
    eq_layout: Vec<EqBandConfig>,
    pcm_pool: PcmPool,
    sample_rate: u32,
) -> Result<PlayerId, SessionError> {
    let sample_rate =
        NonZeroU32::new(sample_rate).ok_or(SessionError::InvalidSampleRate(sample_rate))?;
    let player_id = state.next_player_id;
    let next_player_id = player_id
        .checked_add(1)
        .ok_or(SessionError::PlayerIdExhausted)?;
    let host_id = if state.host.is_none() {
        Some(AssetMapRegistry::reserve_id()?)
    } else {
        None
    };
    let deck_id = AssetMapRegistry::reserve_id()?;
    let deck = Deck::new(player_id, deck_id, bus, eq_layout, pcm_pool, sample_rate);
    if let Some(host) = state.host.as_mut() {
        attach_deck(host, deck)?;
    } else {
        let host_id = host_id
            .ok_or_else(|| SessionError::Graph("session host identity is missing".to_owned()))?;
        let mut host = Host::new(host_id, sample_rate);
        attach_deck(&mut host, deck)?;
        let mut generation = HostMapGeneration::new(host_id);
        generation.commit_revision(BeatMapRevision::first());
        state.host_map_generation = Some(generation);
        state.host = Some(host);
    }
    state.next_player_id = next_player_id;
    debug!(
        player_id,
        players = state.host.as_ref().map_or(0, Host::deck_count),
        "[KITHARA-ROUTE] session player registered"
    );
    Ok(player_id)
}

fn attach_deck(host: &mut Host, deck: Deck) -> Result<(), SessionError> {
    let base = host.topology()?.stamp();
    let admission = host
        .transact(SyncOperation::Topology {
            base,
            operations: Box::new([TopologyOperation::Attach {
                member: SyncMember::Group {
                    alignment: None,
                    group: Box::new(deck),
                },
            }]),
        })
        .map_err(|rejected| <(SyncError, SyncOperation<Deck>)>::from(rejected).0)?;
    if !matches!(admission, SyncAdmission::TopologyChanged { .. }) {
        return Err(SessionError::Graph(
            "session host did not publish the registered deck".to_owned(),
        ));
    }
    Ok(())
}

pub(super) fn detach_deck<B: AudioBackend>(
    state: &mut SessionState<B>,
    deck_id: BeatMapId,
) -> Result<(), SessionError> {
    let host = state
        .host
        .as_mut()
        .ok_or_else(|| SessionError::Graph("session host group is missing".to_owned()))?;
    let base = host.topology()?.stamp();
    let admission = host
        .transact(SyncOperation::Topology {
            base,
            operations: Box::new([TopologyOperation::Detach { member: deck_id }]),
        })
        .map_err(|rejected| <(SyncError, SyncOperation<Deck>)>::from(rejected).0)?;
    if matches!(admission, SyncAdmission::TopologyChanged { .. }) {
        Ok(())
    } else {
        Err(SessionError::Graph(
            "session host did not detach the unregistered deck".to_owned(),
        ))
    }
}

fn ensure_sync_root<B: AudioBackend>(
    state: &mut SessionState<B>,
    sample_rate: NonZeroU32,
) -> Result<(), SessionError> {
    if state.host.is_some() {
        return Ok(());
    }
    let id = AssetMapRegistry::reserve_id()?;
    let host = Host::new(id, sample_rate);
    let mut generation = HostMapGeneration::new(id);
    generation.commit_revision(BeatMapRevision::first());
    state.host_map_generation = Some(generation);
    state.host = Some(host);
    Ok(())
}

pub(super) fn ensure_ctx<B: AudioBackend>(
    state: &mut SessionState<B>,
    sample_rate: u32,
) -> Result<(), SessionError> {
    let map_sample_rate = NonZeroU32::new(sample_rate)
        .or_else(|| NonZeroU32::new(state.sample_rate_hint))
        .ok_or(SessionError::InvalidSampleRate(sample_rate))?;
    ensure_sync_root(state, map_sample_rate)?;
    ensure_stream_ready(state, sample_rate)?;
    ensure_session_output(state)
}

fn ensure_stream_ready<B: AudioBackend>(
    state: &mut SessionState<B>,
    sample_rate: u32,
) -> Result<(), SessionError> {
    if state.ctx.is_none() {
        return create_firewheel_context(state, sample_rate);
    }

    if state.stream_needs_restart {
        debug!(
            sample_rate,
            "[KITHARA-ROUTE] ensuring stopped stream is restarted"
        );
        restart_stream(state, sample_rate)?;
    }

    Ok(())
}

fn create_firewheel_context<B: AudioBackend>(
    state: &mut SessionState<B>,
    sample_rate: u32,
) -> Result<(), SessionError> {
    debug!(sample_rate, "[KITHARA-ROUTE] creating firewheel context");
    let config = FirewheelConfig {
        num_graph_outputs: ChannelCount::STEREO,
        ..FirewheelConfig::default()
    };
    let mut ctx = FirewheelCtx::<B>::new(config);
    let host_map = state
        .host_map_generation
        .take()
        .ok_or_else(|| SessionError::Graph("session host map generation is missing".to_owned()))?;
    let transport_control = match install(&mut ctx, host_map) {
        Ok(control) => control,
        Err(error) => {
            state.host_map_generation = Some(host_map);
            return Err(SessionError::Graph(error.into()));
        }
    };
    if let Err(error) = (state.start_stream_fn)(&mut ctx, sample_rate) {
        state.host_map_generation = Some(host_map);
        return Err(SessionError::StreamStart(error));
    }
    state.ctx = Some(ctx);
    state.transport_control = Some(transport_control);
    state.sample_rate_hint = sample_rate;
    state.stream_needs_restart = false;
    trace_stream_info(state, "start-stream");
    debug!(sample_rate, "[KITHARA-ROUTE] firewheel context ready");
    Ok(())
}

fn ensure_session_output<B: AudioBackend>(state: &mut SessionState<B>) -> Result<(), SessionError> {
    if state.session_output_node_id.is_none() {
        return create_session_output(state);
    }

    Ok(())
}

fn create_session_output<B: AudioBackend>(state: &mut SessionState<B>) -> Result<(), SessionError> {
    debug!("[KITHARA-ROUTE] creating session output graph");
    let Some(ref mut fw_ctx) = state.ctx else {
        return Err(SessionError::NoContext);
    };
    let session_node = VolumeNode::from_linear(ducking_gain(state.session_ducking));
    let session_memo = Memo::new(session_node);
    let session_id = fw_ctx.add_node(session_node, None);
    let limiter_id = fw_ctx.add_node(LimiterNode, None);
    let graph_out = fw_ctx.graph_out_node_id();
    fw_ctx
        .connect(session_id, limiter_id, &[(0, 0), (1, 1)], false)
        .map_err(|err| {
            SessionError::Graph(format!("connect session output to limiter failed: {err}"))
        })?;
    fw_ctx
        .connect(limiter_id, graph_out, &[(0, 0), (1, 1)], false)
        .map_err(|err| {
            SessionError::Graph(format!("connect limiter to graph_out failed: {err}"))
        })?;
    if let Err(err) = fw_ctx.update() {
        warn!("session graph update after output init failed: {err:?}");
    }
    state.session_output_node_id = Some(session_id);
    state.session_output_memo = Some(session_memo);
    state.session_limiter_node_id = Some(limiter_id);
    tap::install_requested(state, limiter_id)?;
    debug!(
        ?session_id,
        ?limiter_id,
        "[KITHARA-ROUTE] session output graph ready"
    );
    Ok(())
}
