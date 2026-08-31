use std::num::NonZeroU32;

use firewheel::{FirewheelCtx, backend::AudioBackend};
use kithara_audio::ConsumerWakeMode;
use kithara_bufpool::{HasPool, PoolRegion};
#[cfg(test)]
use kithara_bufpool::{OverallBudget, PoolConfig, testing::TestPools};
use kithara_platform::sync::Arc;
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

#[cfg(test)]
pub type HostTestPools = TestPools;

#[cfg(test)]
pub(crate) fn pools() -> PoolRegion<HostTestPools> {
    HostTestPools::region(
        OverallBudget(64 * 1024 * 1024),
        PoolConfig::builder().max_buffers(32).build(),
        PoolConfig::builder().max_buffers(128).build(),
    )
    .unwrap_or_else(|error| panic!("host test pool region: {error}"))
}

/// Probe-only access to session-output policy.
pub trait HostProbe {
    /// # Errors
    /// Returns an error when the Host cannot read the output policy.
    fn ducking(&self) -> Result<SessionDuckingMode, PlayError>;

    /// # Errors
    /// Returns an error when the Host rejects the output-policy update.
    fn set_ducking(&self, mode: SessionDuckingMode) -> Result<(), PlayError>;
}

impl<S> HostProbe for Host<S> {
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
pub struct GraphSession<B: AudioBackend, S> {
    state: SessionState<B, S>,
}

impl<B, S> GraphSession<B, S>
where
    B: AudioBackend,
    S: HasPool<f32> + Send + Sync + 'static,
{
    pub const DEFAULT_SAMPLE_RATE: u32 = 44_100;

    #[must_use]
    pub fn new<F>(start_stream_fn: F) -> Self
    where
        F: FnMut(&mut FirewheelCtx<B>, u32) -> Result<(), String> + Send + 'static,
    {
        Self {
            state: state_for(start_stream_fn),
        }
    }

    #[must_use]
    pub fn exec(&mut self, cmd: Cmd<S>) -> Reply {
        if let Cmd::RegisterPlayer { grid_id, pools, .. } = &cmd
            && self.state.root.with_group(*grid_id, |_| ()).is_none()
        {
            attach_player_with_id(&mut self.state, *grid_id, pools.clone());
        }
        run_cmd(&mut self.state, cmd)
    }

    pub fn ctx_mut(&mut self) -> Option<&mut FirewheelCtx<B>> {
        self.state.ctx.as_mut()
    }
}

struct FixtureSession;

impl<S> SessionDispatcher<S> for FixtureSession {
    fn exec(&self, _cmd: Cmd<S>) -> Result<Reply, PlayError> {
        Ok(Reply::Ok)
    }

    fn consumer_wake_mode(&self) -> ConsumerWakeMode {
        ConsumerWakeMode::RealtimeDeferred
    }
}

#[cfg(test)]
pub(crate) fn state<B, F>(start_stream_fn: F) -> SessionState<B, HostTestPools>
where
    B: AudioBackend,
    F: FnMut(&mut FirewheelCtx<B>, u32) -> Result<(), String> + Send + 'static,
{
    state_for(start_stream_fn)
}

fn state_for<B, F, S>(start_stream_fn: F) -> SessionState<B, S>
where
    B: AudioBackend,
    F: FnMut(&mut FirewheelCtx<B>, u32) -> Result<(), String> + Send + 'static,
{
    let grid_id = BeatGridId::allocate().expect("fixture host grid id");
    let sample_rate =
        NonZeroU32::new(SessionState::<B, S>::DEFAULT_SAMPLE_RATE).expect("fixture sample rate");
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
pub(crate) fn attach_player<B: AudioBackend>(
    state: &mut SessionState<B, HostTestPools>,
) -> BeatGridId {
    let grid_id = BeatGridId::allocate().expect("fixture player grid id");
    attach_player_with_id(state, grid_id, pools());
    grid_id
}

fn attach_player_with_id<B, S>(
    state: &mut SessionState<B, S>,
    grid_id: BeatGridId,
    pools: PoolRegion<S>,
) where
    B: AudioBackend,
    S: HasPool<f32> + Send + Sync + 'static,
{
    let worker = PlayWorker::new(PlayWorkerConfig::builder(pools).build());
    let player = PlayerImpl::new(
        PlayerConfig::builder()
            .grid_id(grid_id)
            .worker(worker)
            .session(Arc::new(FixtureSession))
            .build(),
    );
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
                    group: Box::new(PlayerMember::new(player)),
                },
            }]),
        })
        .expect("fixture player attachment");
    assert!(matches!(admission, SyncAdmission::TopologyChanged { .. }));
    state.publish_root();
}
