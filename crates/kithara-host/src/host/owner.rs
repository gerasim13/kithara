use std::{marker::PhantomData, ops::Deref};

use kithara_bufpool::HasPool;
use kithara_command::{Live, When};
use kithara_config::{ConfigOwner, Configure};
use kithara_output::OutputGroup;
use kithara_platform::sync::Arc;
use kithara_play::{PlayError, SessionBinding, SessionDispatcher, player::PlayerControlSource};
use kithara_signal::SessionFrame;
use kithara_warp::{BeatGrid, BeatGridId};

#[cfg(feature = "offline")]
use super::offline::OfflineRuntime;
use super::{
    HostConfig, HostSettings, HostSettingsChange,
    platform::{Platform, PlatformResult},
};
use crate::{
    api::Tap,
    rt::SessionOutput,
    session::{
        HostCmd, HostDispatcher, HostReply, HostRoot, RootView, SessionError, SessionSampleRate,
    },
};

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

impl<P: PlayerControlSource> Deref for HostOwned<P> {
    type Target = P::Control;

    fn deref(&self) -> &Self::Target {
        &self.control
    }
}

/// Exclusive owner and dispatcher for one multi-player output session.
pub struct Host<S> {
    pub(super) dispatcher: Arc<dyn HostDispatcher<S>>,
    pub(super) id: BeatGridId,
    pub(super) root_view: RootView,
    pub(super) session: SessionRuntime<S>,
    pub(super) owns_session: bool,
}

pub(super) enum SessionRuntime<S> {
    Realtime(Platform<S>),
    #[cfg(feature = "offline")]
    Offline {
        platform: Platform<S>,
        runtime: OfflineRuntime<S>,
    },
}

impl<S> SessionRuntime<S> {
    #[cfg(feature = "offline")]
    const fn offline(platform: Platform<S>, runtime: OfflineRuntime<S>) -> Self {
        Self::Offline { platform, runtime }
    }

    /// The offline runtime beside the platform holding its decks.
    #[cfg(feature = "offline")]
    pub(super) const fn offline_mut(&mut self) -> Option<(&Platform<S>, &mut OfflineRuntime<S>)> {
        match self {
            Self::Offline { platform, runtime } => Some((platform, runtime)),
            Self::Realtime(_) => None,
        }
    }

    pub(super) const fn platform(&self) -> &Platform<S> {
        match self {
            Self::Realtime(platform) => platform,
            #[cfg(feature = "offline")]
            Self::Offline { platform, .. } => platform,
        }
    }

    pub(super) const fn realtime(platform: Platform<S>) -> Self {
        Self::Realtime(platform)
    }
}

pub(super) struct SessionRoot {
    pub(super) id: BeatGridId,
    pub(super) root: HostRoot,
    pub(super) view: RootView,
}

impl<S> Host<S> {
    /// Binds `player` to this Host's session and starts its deck there,
    /// seating the player on the slot the session built. Answers the identity
    /// its deck registered under and its control.
    pub(super) fn bind_player<P>(
        &self,
        player: &mut P,
    ) -> Result<(BeatGridId, P::Control), PlayError>
    where
        P: PlayerControlSource<Schema = S>,
    {
        let dispatcher: Arc<dyn SessionDispatcher<S>> = self.dispatcher.clone();
        let registration = player.attach_session(SessionBinding::new(
            dispatcher,
            self.settings().sample_rate(),
        ))?;
        let grid_id = registration.grid_id;
        let slot = self.dispatcher.attach(registration)?;
        player.seat(slot);
        Ok((grid_id, player.control()))
    }

    delegate::delegate! {
        to self.root_view {
            /// Whether the session holds no deck.
            #[must_use]
            pub fn is_empty(&self) -> bool;
            /// The rate the output runs at as measured, beside the rate the
            /// settings ask for, as the session last published them.
            #[must_use]
            #[call(sample_rate)]
            pub fn output_sample_rate(&self) -> SessionSampleRate;
        }
    }

    fn exec_host_ok(&self, cmd: HostCmd<S>, what: &'static str) -> Result<(), PlayError> {
        match self.dispatcher.exec_host(cmd).map_err(PlayError::from)? {
            HostReply::Ok => Ok(()),
            HostReply::Err(error) => Err(error),
            HostReply::Attached(_) => Err(PlayError::Internal(format!(
                "unexpected host reply for {what}"
            ))),
        }
    }

    /// Attaches one output group to `tap`.
    ///
    /// # Errors
    ///
    /// Returns an error when `tap` already has a consumer or graph dispatch fails.
    pub fn attach_outputs(&self, tap: Tap, outputs: OutputGroup) -> Result<(), PlayError> {
        self.exec_host_ok(HostCmd::AttachOutputs { tap, outputs }, "output attach")
    }

    /// Removes the output group attached to `tap`, if any.
    ///
    /// # Errors
    ///
    /// Returns an error when graph dispatch fails.
    pub fn detach_outputs(&self, tap: Tap) -> Result<(), PlayError> {
        self.exec_host_ok(HostCmd::DetachOutputs { tap }, "output detach")
    }

    /// Restarts the output on the platform's new route, keeping Host-owned
    /// graph state, and tells every deck's listeners the route changed.
    ///
    /// # Errors
    /// Returns an error when the session cannot restart its output route.
    pub fn invalidate_audio_route<R>(&self, reason: R) -> Result<(), PlayError>
    where
        R: Into<String>,
    {
        self.exec_host_ok(
            HostCmd::InvalidateAudioRoute {
                reason: reason.into(),
            },
            "route invalidation",
        )
    }

    pub(super) fn owned<P>(&self, id: BeatGridId, control: P::Control) -> HostOwned<P>
    where
        P: PlayerControlSource,
    {
        HostOwned {
            id,
            control,
            host_id: self.id,
            marker: PhantomData,
        }
    }

    fn owner(
        id: BeatGridId,
        root_view: RootView,
        dispatcher: Arc<dyn HostDispatcher<S>>,
        session: SessionRuntime<S>,
    ) -> Self {
        Self {
            id,
            root_view,
            dispatcher,
            session,
            owns_session: true,
        }
    }

    pub(super) fn session_root(settings: HostSettings) -> Result<SessionRoot, PlayError> {
        let grid_id = BeatGridId::allocate().map_err(SessionError::from)?;
        let root = HostRoot::new(grid_id, settings.sample_rate());
        let view = RootView::new(&root, settings);
        Ok(SessionRoot {
            root,
            view,
            id: grid_id,
        })
    }

    pub(super) fn validate_removal<P>(&self, player: &HostOwned<P>) -> Result<(), PlayError>
    where
        P: PlayerControlSource<Schema = S>,
        S: Send + Sync + 'static,
    {
        if player.host_id != self.id {
            return Err(PlayError::ForeignSession);
        }
        if self.root_view.holds(player.id()) {
            return Ok(());
        }
        Err(SessionError::DeckNotFound(player.id()).into())
    }
}

impl<S> Host<S>
where
    S: HasPool<f32> + Send + Sync + 'static,
{
    /// Creates one Host with its configured realtime or offline session.
    ///
    /// # Errors
    /// Returns [`PlayError::InvalidParameter`] naming the first setting
    /// out of its bounds, or an error when the session root or selected
    /// runtime cannot start.
    pub fn new(config: HostConfig<S>) -> Result<Self, PlayError> {
        match config {
            HostConfig::Realtime {
                output_block_frames,
                limiter,
                settings,
                ..
            } => {
                let settings = Live::new(settings)?;
                let root = Self::session_root(*settings.config())?;
                let (dispatcher, platform) = Platform::realtime(
                    root.root,
                    root.view.clone(),
                    output_block_frames,
                    SessionOutput::new(limiter),
                    settings,
                )
                .resolve()?;
                Ok(Self::owner(
                    root.id,
                    root.view,
                    dispatcher,
                    SessionRuntime::realtime(platform),
                ))
            }
            #[cfg(feature = "offline")]
            config @ HostConfig::Offline { .. } => {
                let root = Self::session_root(config.settings())?;
                let (dispatcher, platform, runtime) =
                    Platform::offline(config, root.root, root.view.clone())?;
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

/// A change goes to the session owner, which hands it to the render graph to
/// apply on a session frame; the settings show it once the graph confirms it.
impl<S> Configure<HostSettingsChange> for Host<S> {
    type At = When<SessionFrame>;
    type Config = HostSettings;
    type Error = PlayError;
    type Output = ();

    fn configure(&self, change: HostSettingsChange, at: Self::At) -> Result<(), PlayError> {
        self.exec_host_ok(HostCmd::Configure { change, at }, "host settings")
    }

    fn settings(&self) -> HostSettings {
        self.root_view.settings()
    }
}

impl<S> Drop for Host<S> {
    fn drop(&mut self) {
        #[cfg(target_arch = "wasm32")]
        self.session.platform().close(self.id);
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

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use kithara_test_utils::{bufpool::TestPools, kithara};

    use super::*;

    #[kithara::test]
    fn realtime_config_preserves_output_block_default_and_allows_override() {
        let default = HostConfig::<TestPools>::builder().build();
        let HostConfig::Realtime {
            output_block_frames,
            ..
        } = default
        else {
            panic!("default Host config must be realtime");
        };
        assert_eq!(output_block_frames, None);

        let frames = NonZeroU32::new(128).expect("test block size is non-zero");
        let configured = HostConfig::<TestPools>::builder()
            .output_block_frames(frames)
            .build();
        let HostConfig::Realtime {
            output_block_frames,
            ..
        } = configured
        else {
            panic!("realtime builder must create realtime config");
        };
        assert_eq!(output_block_frames, Some(frames));
    }

    #[kithara::test]
    fn host_root_owns_the_configured_sample_rate() {
        let sample_rate = NonZeroU32::new(48_000).expect("test sample rate is non-zero");
        let config = HostConfig::<TestPools>::builder()
            .settings(HostSettings::builder().sample_rate(sample_rate).build())
            .build();
        let root = Host::<TestPools>::session_root(config.settings()).expect("host root");

        assert_eq!(root.view.grid().axis().sample_rate(), sample_rate);
    }
}
