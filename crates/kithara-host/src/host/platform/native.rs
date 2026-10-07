use std::{marker::PhantomData, num::NonZeroU32};

use kithara_bufpool::HasPool;
use kithara_command::Live;
use kithara_platform::sync::Arc;
use kithara_play::{PlayError, player::PlayerControlSource};

#[cfg(feature = "offline")]
use super::super::{
    HostConfig,
    offline::{OfflineRuntime, StartedOffline},
};
use super::{
    super::{Host, HostOwned},
    PlatformResult,
};
use crate::{
    HostSettings,
    rt::SessionOutput,
    session::{HostDispatcher, HostProtocol, HostRoot, RootView, decks::SessionDecks},
};

type StartedPlatform<S> = (Arc<dyn HostDispatcher<S>>, Platform<S>);

impl<S> PlatformResult<Self> for StartedPlatform<S> {
    fn resolve(self) -> Result<Self, PlayError> {
        Ok(self)
    }
}

/// A native Host reaches the decks its session thread holds.
pub(in crate::host) struct Platform<S> {
    decks: SessionDecks,
    marker: PhantomData<fn() -> S>,
}

impl<S> Platform<S> {
    /// Starts an offline session whose task holds the Host's decks.
    #[cfg(feature = "offline")]
    pub(in crate::host) fn offline(
        config: HostConfig<S>,
        root: HostRoot,
        view: RootView,
    ) -> Result<StartedOffline<S>, PlayError>
    where
        S: HasPool<f32> + Send + Sync + 'static,
    {
        let (dispatcher, runtime) = OfflineRuntime::new(config, root, view)?;
        let platform = Self::owner(SessionDecks::new(runtime.deck_inbox()));
        Ok((dispatcher, platform, runtime))
    }

    const fn owner(decks: SessionDecks) -> Self {
        Self {
            decks,
            marker: PhantomData,
        }
    }

    /// Ticks the decks of an offline session ahead of one rendered block.
    #[cfg(feature = "offline")]
    pub(in crate::host) fn tick_block(&self) {
        self.decks.tick_block();
    }

    pub(in crate::host) fn realtime(
        root: HostRoot,
        view: RootView,
        output_block_frames: Option<NonZeroU32>,
        output: SessionOutput,
        settings: Live<HostSettings, HostProtocol>,
    ) -> StartedPlatform<S>
    where
        S: HasPool<f32> + Send + Sync + 'static,
    {
        let session =
            crate::session::native::spawn::<S>(root, view, output_block_frames, output, settings);
        let decks = SessionDecks::new(session.clone());
        (session, Self::owner(decks))
    }
}

impl<S> Host<S>
where
    S: HasPool<f32> + Send + Sync + 'static,
{
    /// Attaches and transfers one fully configured player or decorator into
    /// this Host, which starts its deck before returning: the output runs
    /// from now until the Host hands its last deck back. Audio-device setup
    /// may block; musical playback remains stopped.
    ///
    /// # Errors
    /// Returns an error when binding, attachment, or starting the deck fails.
    pub fn insert<P>(&mut self, mut player: P) -> Result<HostOwned<P>, PlayError>
    where
        P: PlayerControlSource<Schema = S>,
    {
        let (grid_id, control) = self.bind_player(&mut player)?;
        if let Err(error) = self
            .session
            .platform()
            .decks
            .hold(grid_id, Box::new(player))
        {
            self.dispatcher.detach(grid_id)?;
            return Err(error);
        }
        Ok(self.owned::<P>(grid_id, control))
    }

    /// Closes the lower runtime on the caller thread, stops its deck and
    /// detaches it from the session, then takes the deck back and drops it.
    /// The last deck the Host hands back releases the output.
    ///
    /// # Errors
    /// Returns an error when close or canonical detachment fails.
    pub fn remove<P>(&mut self, player: &HostOwned<P>) -> Result<(), PlayError>
    where
        P: PlayerControlSource<Schema = S>,
    {
        self.validate_removal(player)?;
        P::close_control(player.control())?;
        self.dispatcher.detach(player.id())?;
        self.session.platform().decks.release(player.id()).map(drop)
    }
}
