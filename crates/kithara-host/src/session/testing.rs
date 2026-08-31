use std::num::NonZeroU32;

use firewheel::{FirewheelCtx, backend::AudioBackend};
use kithara_audio::ConsumerWakeMode;
use kithara_bufpool::{BytePool, SamplePool};
use kithara_platform::sync::Arc;
#[cfg(target_arch = "wasm32")]
use kithara_play::player::PlayerControlSource;
use kithara_play::{
    GroupState, PlayError, PlayWorker, PlayWorkerConfig, PlayerConfig, PlayerImpl,
    SessionDuckingMode, player::PlayerMember,
};
use kithara_warp::{
    BeatGridId, SessionEpoch, SyncAdmission, SyncGroup, SyncMember, SyncMemberKind, SyncOperation,
    TopologyOperation,
};

use super::{
    dispatch::run_cmd,
    protocol::{Cmd, Reply, SessionDispatcher},
    state::{RootView, SessionState},
};
use crate::Host;

/// Probe-only access to session-output policy.
pub trait HostProbe {
    /// # Errors
    /// Returns an error when the Host cannot read the output policy.
    fn ducking(&self) -> Result<SessionDuckingMode, PlayError>;

    /// # Errors
    /// Returns an error when the Host rejects the output-policy update.
    fn set_ducking(&self, mode: SessionDuckingMode) -> Result<(), PlayError>;
}

impl HostProbe for Host {
    delegate::delegate! {
        to self {
            #[call(ducking_mode)]
            fn ducking(&self) -> Result<SessionDuckingMode, PlayError>;
            #[call(set_ducking_mode)]
            fn set_ducking(&self, mode: SessionDuckingMode) -> Result<(), PlayError>;
        }
    }
}

/// Test-only owner for the real Host graph running on an injected backend.
///
/// The production Host surface never exposes its raw session state. This
/// probe keeps existing deterministic backend tests on the same graph code.
pub struct GraphSession<B: AudioBackend> {
    state: SessionState<B>,
}

impl<B: AudioBackend> GraphSession<B> {
    pub const DEFAULT_SAMPLE_RATE: u32 = 44_100;

    #[must_use]
    pub fn new<F>(start_stream_fn: F) -> Self
    where
        F: FnMut(&mut FirewheelCtx<B>, u32) -> Result<(), String> + Send + 'static,
    {
        Self {
            state: state(start_stream_fn),
        }
    }

    #[must_use]
    pub fn exec(&mut self, cmd: Cmd) -> Reply {
        if let Cmd::RegisterPlayer { grid_id, .. } = &cmd
            && self.state.root.with_group(*grid_id, |_| ()).is_none()
        {
            attach_player_with_id(&mut self.state, *grid_id);
        }
        run_cmd(&mut self.state, cmd)
    }

    pub fn ctx_mut(&mut self) -> Option<&mut FirewheelCtx<B>> {
        self.state.ctx.as_mut()
    }
}

struct FixtureSession;

impl SessionDispatcher for FixtureSession {
    fn exec(&self, _cmd: Cmd) -> Result<Reply, PlayError> {
        Ok(Reply::Ok)
    }

    fn consumer_wake_mode(&self) -> ConsumerWakeMode {
        ConsumerWakeMode::RealtimeDeferred
    }
}

pub(crate) fn state<B, F>(start_stream_fn: F) -> SessionState<B>
where
    B: AudioBackend,
    F: FnMut(&mut FirewheelCtx<B>, u32) -> Result<(), String> + Send + 'static,
{
    let grid_id = BeatGridId::allocate().expect("fixture host grid id");
    let sample_rate =
        NonZeroU32::new(SessionState::<B>::DEFAULT_SAMPLE_RATE).expect("fixture sample rate");
    let root = GroupState::unavailable(
        grid_id,
        sample_rate,
        SessionEpoch::new(0),
        SyncMemberKind::Group,
    );
    let root_view = RootView::new(&root);
    SessionState::new(root, root_view, sample_rate, start_stream_fn)
}

#[cfg(test)]
pub(crate) fn attach_player<B: AudioBackend>(state: &mut SessionState<B>) -> BeatGridId {
    let grid_id = BeatGridId::allocate().expect("fixture player grid id");
    attach_player_with_id(state, grid_id);
    grid_id
}

fn attach_player_with_id<B: AudioBackend>(state: &mut SessionState<B>, grid_id: BeatGridId) {
    let member = fixture_member(grid_id);
    let base = state
        .root
        .topology()
        .expect("fixture host topology")
        .stamp();
    let admission = state
        .root
        .transact(SyncOperation::Topology {
            base,
            operations: Box::new([TopologyOperation::Attach {
                member: SyncMember::Group {
                    alignment: None,
                    group: Box::new(member),
                },
            }]),
        })
        .expect("fixture player attachment");
    assert!(matches!(admission, SyncAdmission::TopologyChanged { .. }));
    state.publish_root();
}

pub(crate) fn fixture_member(grid_id: BeatGridId) -> PlayerMember {
    let worker = PlayWorker::new(
        PlayWorkerConfig::for_pools(BytePool::default(), SamplePool::default()).build(),
    );
    let player = PlayerImpl::new(
        PlayerConfig::builder()
            .grid_id(grid_id)
            .worker(worker)
            .session(Arc::new(FixtureSession))
            .build(),
    );
    target_member(player)
}

#[cfg(not(target_arch = "wasm32"))]
fn target_member(player: PlayerImpl) -> PlayerMember {
    PlayerMember::new(player)
}

#[cfg(target_arch = "wasm32")]
fn target_member(mut player: PlayerImpl) -> PlayerMember {
    player
        .take_host_member()
        .expect("fixture player synchronization member")
}
