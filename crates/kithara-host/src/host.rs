use std::{marker::PhantomData, num::NonZeroU32, ops::Deref};

use bon::Builder;
use kithara_bufpool::HasPool;
use kithara_platform::sync::Arc;
#[cfg(any(test, feature = "probe"))]
use kithara_play::TransportRevision;
use kithara_play::{
    GroupState, PlayError, SessionBinding, SessionDispatcher, Tempo,
    player::{PlayerControlSource, PlayerMember},
};
use kithara_warp::{
    BeatGrid, BeatGridId, SessionEpoch, SyncAdmission, SyncApplied, SyncError, SyncGroup,
    SyncGroupSnapshot, SyncMember, SyncMemberKind, SyncOperation, SyncRejected, SyncStatusSnapshot,
    TopologyOperation,
};

#[cfg(feature = "offline")]
mod offline;
mod platform;

#[cfg(feature = "offline")]
pub use offline::OfflineSessionConfig;
use platform::{Platform, PlatformResult};

#[cfg(any(test, feature = "probe"))]
use crate::api::SessionDuckingMode;
use crate::{
    api::HostLevel,
    bridge::MixTapWriter,
    session::{
        Cmd, HostCmd, HostDispatcher, HostReply, Reply, RootView, SessionError, SessionSampleRate,
    },
};

const DEFAULT_SAMPLE_RATE: NonZeroU32 = match NonZeroU32::new(44_100) {
    Some(sample_rate) => sample_rate,
    None => unreachable!(),
};

/// Configuration for a platform realtime session.
#[derive(Clone, Copy, Builder, fieldwork::Fieldwork)]
#[builder(state_mod(vis = "pub"))]
#[fieldwork(opt_in, get)]
#[non_exhaustive]
pub struct RealtimeSessionConfig {
    /// Initial device-rate hint. Physical route changes may update it later.
    #[builder(default = DEFAULT_SAMPLE_RATE)]
    #[field(get, copy)]
    sample_rate_hint: NonZeroU32,
}

impl Default for RealtimeSessionConfig {
    fn default() -> Self {
        Self::builder().build()
    }
}

/// Runtime mode and configuration for one Host-owned session.
#[non_exhaustive]
pub enum SessionConfig<S> {
    /// Device-backed platform session.
    Realtime(RealtimeSessionConfig),
    /// Device-free finite renderer.
    #[cfg(feature = "offline")]
    Offline(Box<OfflineSessionConfig<S>>),
}

impl<S> SessionConfig<S> {
    /// Configure a platform realtime session.
    #[must_use]
    pub const fn realtime(config: RealtimeSessionConfig) -> Self {
        Self::Realtime(config)
    }
}

impl<S> Default for SessionConfig<S> {
    fn default() -> Self {
        Self::Realtime(RealtimeSessionConfig::default())
    }
}

/// Configuration for the shared output session owned by [`Host`].
#[derive(Builder, fieldwork::Fieldwork)]
#[builder(state_mod(vis = "pub"))]
#[fieldwork(opt_in, get)]
#[non_exhaustive]
pub struct HostConfig<S> {
    /// Realtime or offline session selected at Host construction.
    #[builder(default)]
    #[field(get)]
    session: SessionConfig<S>,
}

/// Typed command proxy for one player value exclusively resident in a Host.
#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub struct HostOwned<P: PlayerControlSource> {
    host_id: BeatGridId,
    #[field(get, copy)]
    id: BeatGridId,
    #[field(get)]
    control: P::Control,
    marker: PhantomData<fn() -> P>,
}

impl<P: PlayerControlSource> HostOwned<P> {
    /// Creates one input for [`Host::apply_mix`].
    #[must_use]
    pub const fn level(&self, level: f32) -> HostLevel {
        HostLevel::new(self.id, level)
    }
}

impl<P: PlayerControlSource> Deref for HostOwned<P> {
    type Target = P::Control;

    fn deref(&self) -> &Self::Target {
        &self.control
    }
}

/// Exclusive owner and dispatcher for one multi-player output session.
pub struct Host<S> {
    id: BeatGridId,
    owns_session: bool,
    root_view: RootView,
    dispatcher: Arc<dyn HostDispatcher<S>>,
    session: SessionRuntime<S>,
}

enum SessionRuntime<S> {
    Realtime(Platform<S>),
    #[cfg(feature = "offline")]
    Offline {
        platform: Platform<S>,
        runtime: offline::OfflineRuntime<S>,
    },
}

impl<S> SessionRuntime<S> {
    const fn realtime(platform: Platform<S>) -> Self {
        Self::Realtime(platform)
    }

    #[cfg(feature = "offline")]
    const fn offline(platform: Platform<S>, runtime: offline::OfflineRuntime<S>) -> Self {
        Self::Offline { platform, runtime }
    }

    const fn platform(&self) -> &Platform<S> {
        match self {
            Self::Realtime(platform) => platform,
            #[cfg(feature = "offline")]
            Self::Offline { platform, .. } => platform,
        }
    }

    const fn platform_mut(&mut self) -> &mut Platform<S> {
        match self {
            Self::Realtime(platform) => platform,
            #[cfg(feature = "offline")]
            Self::Offline { platform, .. } => platform,
        }
    }

    #[cfg(feature = "offline")]
    const fn offline_runtime_mut(&mut self) -> Option<&mut offline::OfflineRuntime<S>> {
        match self {
            Self::Offline { runtime, .. } => Some(runtime),
            Self::Realtime(_) => None,
        }
    }
}

struct SessionRoot {
    id: BeatGridId,
    sample_rate: NonZeroU32,
    group: GroupState<PlayerMember>,
    view: RootView,
}

impl<S> Host<S> {
    fn session_root(sample_rate: NonZeroU32) -> Result<SessionRoot, PlayError> {
        let grid_id = BeatGridId::allocate().map_err(SessionError::from)?;
        let group = GroupState::unavailable(
            grid_id,
            sample_rate,
            SessionEpoch::new(0),
            SyncMemberKind::Group,
        );
        let view = RootView::new(&group);
        Ok(SessionRoot {
            id: grid_id,
            sample_rate,
            group,
            view,
        })
    }

    fn owner(
        id: BeatGridId,
        root_view: RootView,
        dispatcher: Arc<dyn HostDispatcher<S>>,
        session: SessionRuntime<S>,
    ) -> Self {
        Self {
            id,
            owns_session: true,
            root_view,
            dispatcher,
            session,
        }
    }

    fn bind_player<P>(&self, player: &mut P) -> Result<(BeatGridId, P::Control), PlayError>
    where
        P: PlayerControlSource<Schema = S>,
    {
        let grid_id = player.id();
        let dispatcher: Arc<dyn SessionDispatcher<S>> = self.dispatcher.clone();
        player.attach_session(SessionBinding::new(dispatcher))?;
        Ok((grid_id, player.control()))
    }

    fn attach_member(&self, member: PlayerMember) -> Result<(), PlayError> {
        let operations = Box::new([TopologyOperation::Attach {
            member: SyncMember::Group {
                alignment: None,
                group: Box::new(member),
            },
        }]);
        require_topology_change(self.dispatcher.transact_current(operations))
    }

    fn owned<P>(&self, id: BeatGridId, control: P::Control) -> HostOwned<P>
    where
        P: PlayerControlSource,
    {
        HostOwned {
            host_id: self.id,
            id,
            control,
            marker: PhantomData,
        }
    }

    fn validate_removal<P>(&self, player: &HostOwned<P>) -> Result<(), PlayError>
    where
        P: PlayerControlSource<Schema = S>,
        S: Send + Sync + 'static,
    {
        if player.host_id != self.id {
            return Err(PlayError::ForeignSession);
        }
        let topology = self.topology().map_err(SessionError::from)?;
        if topology
            .members()
            .iter()
            .any(|member| member.grid().id() == player.id())
        {
            return Ok(());
        }
        Err(SessionError::from(SyncError::MemberNotFound {
            group_id: self.id,
            member_id: player.id(),
        })
        .into())
    }

    fn detach_member(&self, member: BeatGridId) -> Result<(), PlayError> {
        let operations = Box::new([TopologyOperation::Detach { member }]);
        require_topology_change(self.dispatcher.transact_current(operations))
    }

    /// Applies one validated, atomic batch of final player levels.
    ///
    /// # Errors
    /// Returns an error for invalid members, levels, or graph dispatch failure.
    pub fn apply_mix<I>(&self, levels: I) -> Result<(), PlayError>
    where
        I: IntoIterator<Item = HostLevel>,
    {
        let levels = levels.into_iter().collect();
        match self
            .dispatcher
            .exec_host(HostCmd::ApplyMix { levels })
            .map_err(PlayError::from)?
        {
            HostReply::Ok => Ok(()),
            HostReply::Err(error) => Err(error),
            _ => Err(PlayError::Internal(
                "unexpected host reply for mix update".into(),
            )),
        }
    }

    /// Reads the current output-rate observation without exposing the lower
    /// session handle.
    ///
    /// # Errors
    /// Returns an error when the canonical session cannot answer the query.
    pub fn sample_rate(&self) -> Result<SessionSampleRate, PlayError> {
        match self.dispatcher.exec(Cmd::QuerySampleRate)? {
            Reply::SampleRate(sample_rate) => Ok(sample_rate),
            Reply::Err(error) => Err(error.into()),
            _ => Err(PlayError::Internal(
                "unexpected host reply for sample-rate query".into(),
            )),
        }
    }

    /// Change the canonical session tempo at the next render boundary.
    ///
    /// # Errors
    /// Returns an error when the Host rejects or cannot dispatch the update.
    pub fn set_tempo(&self, tempo: Tempo) -> Result<(), PlayError> {
        self.exec_play_ok(Cmd::SetSessionTempo { tempo })
    }

    /// Read the canonical session transport revision for probes.
    ///
    /// # Errors
    /// Returns an error when the Host cannot answer the query.
    #[cfg(any(test, feature = "probe"))]
    pub(crate) fn transport_revision(&self) -> Result<TransportRevision, PlayError> {
        match self.dispatcher.exec(Cmd::QuerySessionTransport)? {
            Reply::SessionTransport(snapshot) => Ok(snapshot.revision()),
            Reply::Err(error) => Err(error.into()),
            _ => Err(PlayError::Internal(
                "unexpected host reply for transport query".into(),
            )),
        }
    }

    /// Installs the single post-limiter mix tap.
    ///
    /// # Errors
    /// Returns an error when a tap is active or graph dispatch fails.
    pub fn enable_mix_tap(&self, writer: MixTapWriter) -> Result<(), PlayError> {
        self.exec_play_ok(Cmd::EnableMixTap { writer })
    }

    /// Removes the post-limiter mix tap.
    ///
    /// # Errors
    /// Returns an error when graph dispatch fails.
    pub fn disable_mix_tap(&self) -> Result<(), PlayError> {
        self.exec_play_ok(Cmd::DisableMixTap)
    }

    /// Restart the current output route while preserving Host-owned graph state.
    ///
    /// # Errors
    /// Returns an error when the session cannot restart its output route.
    pub fn invalidate_audio_route<R>(&self, reason: R) -> Result<(), PlayError>
    where
        R: Into<String>,
    {
        self.exec_play_ok(Cmd::InvalidateAudioRoute {
            reason: reason.into(),
        })
    }

    /// Updates the shared output-session ducking mode.
    ///
    /// # Errors
    /// Returns an error when the canonical session rejects the update.
    #[cfg(any(test, feature = "probe"))]
    pub(crate) fn set_ducking_mode(&self, mode: SessionDuckingMode) -> Result<(), PlayError> {
        self.exec_play_ok(Cmd::SetSessionDucking { mode })
    }

    /// Reads the shared output-session ducking mode.
    ///
    /// # Errors
    /// Returns an error when the canonical session cannot answer the query.
    #[cfg(any(test, feature = "probe"))]
    pub(crate) fn ducking_mode(&self) -> Result<SessionDuckingMode, PlayError> {
        match self.dispatcher.exec(Cmd::SessionDucking)? {
            Reply::SessionDucking(mode) => Ok(mode),
            Reply::Err(error) => Err(error.into()),
            _ => Err(PlayError::Internal(
                "unexpected host reply for ducking query".into(),
            )),
        }
    }

    fn exec_play_ok(&self, cmd: Cmd<S>) -> Result<(), PlayError> {
        match self.dispatcher.exec(cmd)? {
            Reply::Ok => Ok(()),
            Reply::Err(error) => Err(error.into()),
            _ => Err(PlayError::Internal(
                "unexpected host reply for session command".into(),
            )),
        }
    }
}

impl<S> Host<S>
where
    S: HasPool<f32> + Send + Sync + 'static,
{
    /// Creates one Host with its configured realtime or offline session.
    ///
    /// # Errors
    /// Returns an error when the session root or selected runtime cannot start.
    pub fn new(config: HostConfig<S>) -> Result<Self, PlayError> {
        match config.session {
            SessionConfig::Realtime(config) => {
                let root = Self::session_root(config.sample_rate_hint)?;
                let (dispatcher, platform) =
                    Platform::realtime(root.group, root.view.clone(), root.sample_rate)
                        .resolve()?;
                Ok(Self::owner(
                    root.id,
                    root.view,
                    dispatcher,
                    SessionRuntime::realtime(platform),
                ))
            }
            #[cfg(feature = "offline")]
            SessionConfig::Offline(config) => {
                let platform = Platform::offline().resolve()?;
                let root = Self::session_root(config.sample_rate())?;
                let (dispatcher, runtime) =
                    offline::OfflineRuntime::new(*config, root.group, root.view.clone())?;
                Ok(Self::owner(
                    root.id,
                    root.view,
                    dispatcher,
                    SessionRuntime::offline(platform, runtime),
                ))
            }
        }
    }
}

impl<S> Drop for Host<S> {
    fn drop(&mut self) {
        Platform::close(self.session.platform_mut(), self.id);
        if self.owns_session
            && let Err(error) = self.dispatcher.exec_host(HostCmd::Shutdown)
        {
            tracing::warn!(error = %PlayError::from(error), "host session shutdown failed");
        }
    }
}

impl<S: Send + Sync + 'static> BeatGrid for Host<S> {
    fn id(&self) -> BeatGridId {
        self.id
    }

    fn snapshot(&self) -> kithara_warp::BeatGridSnapshot {
        self.root_view.grid()
    }
}

impl<S: Send + Sync + 'static> SyncGroup for Host<S> {
    type NestedGroup = PlayerMember;

    delegate::delegate! {
        to self.root_view {
            fn topology(&self) -> Result<SyncGroupSnapshot, SyncError>;
            fn status(&self) -> SyncStatusSnapshot;
        }
        to self.dispatcher {
            fn acknowledge(&mut self, applied: SyncApplied) -> Result<SyncStatusSnapshot, SyncError>;
        }
    }

    fn transact(
        &mut self,
        operation: SyncOperation<PlayerMember>,
    ) -> Result<SyncAdmission, SyncRejected<PlayerMember>> {
        Platform::transact(self.session.platform(), &self.dispatcher, operation)
    }
}

fn require_topology_change(result: Result<SyncAdmission, PlayError>) -> Result<(), PlayError> {
    match result {
        Ok(SyncAdmission::TopologyChanged { .. }) => Ok(()),
        Ok(_) => Err(PlayError::Internal(
            "host topology operation did not change topology".into(),
        )),
        Err(error) => Err(error),
    }
}
