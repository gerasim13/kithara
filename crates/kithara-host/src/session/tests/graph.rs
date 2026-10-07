use std::num::NonZeroU32;

use firewheel::FirewheelContext;
use kithara_bufpool::HasPool;
use kithara_command::Live;
use kithara_effects::LimiterConfig;
use kithara_play::SessionTransportSnapshot;
#[cfg(test)]
use kithara_test_utils::bufpool::TestPools;
use kithara_warp::BeatGridId;

use super::super::{
    dispatch::{run_cmd, run_host_cmd},
    protocol::{Cmd, HostCmd, HostReply, Reply},
    state::{HostRoot, RootView, SessionState},
    transport::observe_commits,
};
use crate::{HostSettings, rt::SessionOutput};
/// Test-only owner for the real Host graph running on an injected backend.
///
/// The production Host surface never exposes its raw session state. This
/// probe keeps existing deterministic backend tests on the same graph code.
pub(crate) struct GraphSession<T, S> {
    state: SessionState<T, S>,
}

impl<T, S> GraphSession<T, S>
where
    S: HasPool<f32> + Send + Sync + 'static,
{
    pub(crate) const DEFAULT_SAMPLE_RATE: NonZeroU32 =
        match NonZeroU32::new(SessionState::<T, S>::DEFAULT_SAMPLE_RATE) {
            Some(sample_rate) => sample_rate,
            None => unreachable!(),
        };

    #[must_use]
    pub(crate) fn new<F>(start_stream_fn: F) -> Self
    where
        F: FnMut(&mut FirewheelContext, u32) -> Result<T, String> + Send + 'static,
    {
        Self::with_sample_rate(Self::DEFAULT_SAMPLE_RATE, start_stream_fn)
    }

    pub(crate) fn ctx_mut(&mut self) -> Option<&mut FirewheelContext> {
        self.state.ctx.as_mut()
    }

    #[must_use]
    pub(crate) fn exec(&mut self, cmd: Cmd<S>) -> Reply {
        if let Cmd::RegisterPlayer { grid_id, .. } = &cmd
            && !self.state.root.holds(*grid_id)
        {
            attach_player_with_id(&mut self.state, *grid_id);
        }
        run_cmd(&mut self.state, cmd)
    }

    #[must_use]
    pub(crate) fn exec_host(&mut self, cmd: HostCmd<S>) -> HostReply {
        run_host_cmd(&mut self.state, cmd)
    }

    pub(crate) fn stream_mut(&mut self) -> Option<&mut T> {
        self.state.stream.as_mut()
    }

    /// What the transport last committed; see [`committed_transport`].
    pub(crate) fn transport(&mut self) -> Option<SessionTransportSnapshot> {
        committed_transport(&mut self.state)
    }

    /// The view the session publishes, as its clients read it.
    pub(crate) fn view(&self) -> RootView {
        self.state.root_view.clone()
    }

    #[must_use]
    pub(crate) fn with_sample_rate<F>(sample_rate: NonZeroU32, start_stream_fn: F) -> Self
    where
        F: FnMut(&mut FirewheelContext, u32) -> Result<T, String> + Send + 'static,
    {
        Self {
            state: state_for(sample_rate, start_stream_fn),
        }
    }
}

#[cfg(test)]
pub(crate) fn state<T, F>(start_stream_fn: F) -> SessionState<T, TestPools>
where
    F: FnMut(&mut FirewheelContext, u32) -> Result<T, String> + Send + 'static,
{
    state_for(
        GraphSession::<T, TestPools>::DEFAULT_SAMPLE_RATE,
        start_stream_fn,
    )
}

pub(crate) fn state_for<T, F, S>(sample_rate: NonZeroU32, start_stream_fn: F) -> SessionState<T, S>
where
    F: FnMut(&mut FirewheelContext, u32) -> Result<T, String> + Send + 'static,
{
    let grid_id = BeatGridId::allocate().expect("fixture host grid id");
    let root = HostRoot::new(grid_id, sample_rate);
    let settings = HostSettings::builder().sample_rate(sample_rate).build();
    let root_view = RootView::new(&root, settings);
    SessionState::new(
        root,
        root_view,
        None,
        None,
        SessionOutput::new(LimiterConfig::default()),
        Live::new(settings).expect("the fixture settings are valid"),
        start_stream_fn,
    )
}

#[cfg(test)]
pub(crate) fn attach_player<T>(state: &mut SessionState<T, TestPools>) -> BeatGridId {
    let grid_id = BeatGridId::allocate().expect("fixture player grid id");
    attach_player_with_id(state, grid_id);
    grid_id
}

#[cfg(test)]
pub(crate) fn root_with_player(sample_rate: NonZeroU32) -> (HostRoot, RootView, BeatGridId) {
    let host_grid_id = BeatGridId::allocate().expect("fixture host grid id");
    let mut root = HostRoot::new(host_grid_id, sample_rate);
    let player_grid_id = BeatGridId::allocate().expect("fixture player grid id");
    root.attach(player_grid_id)
        .expect("fixture player attachment");
    let root_view = RootView::new(
        &root,
        HostSettings::builder().sample_rate(sample_rate).build(),
    );
    (root, root_view, player_grid_id)
}

/// Brings the Host grid up to what the render graph committed, then reads
/// that commit; nothing while a route restart holds the session grid.
pub(crate) fn committed_transport<T, S>(
    state: &mut SessionState<T, S>,
) -> Option<SessionTransportSnapshot> {
    observe_commits(state);
    if state.reserved_session_grid.is_some() {
        return None;
    }
    state.transport_control.as_mut()?.observation().snapshot()
}

fn attach_player_with_id<T, S>(state: &mut SessionState<T, S>, grid_id: BeatGridId) {
    state
        .root
        .attach(grid_id)
        .expect("fixture player attachment");
    state.publish_root();
}
