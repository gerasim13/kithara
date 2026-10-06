use std::{marker::PhantomData, num::NonZeroU32};

use kithara_bufpool::HasPool;
use kithara_command::Live;
use kithara_platform::sync::Arc;
use kithara_play::{PlayError, player::PlayerControlSource};
use kithara_warp::BeatGridId;

#[cfg(feature = "offline")]
use super::decks::Decks;
use super::{
    super::{Host, HostOwned},
    PlatformResult,
    deck_pass::{DeckPass, DeckThread},
};
use crate::{
    HostSettings,
    rt::SessionOutput,
    session::{HostDispatcher, HostProtocol, HostRoot, RootView},
};

type StartedPlatform<S> = (Arc<dyn HostDispatcher<S>>, Platform<S>);

impl<S> PlatformResult<Self> for Platform<S> {
    fn resolve(self) -> Result<Self, PlayError> {
        Ok(self)
    }
}

impl<S> PlatformResult<Self> for StartedPlatform<S> {
    fn resolve(self) -> Result<Self, PlayError> {
        Ok(self)
    }
}

pub(in crate::host) struct Platform<S> {
    decks: DeckPass,
    marker: PhantomData<fn() -> S>,
}

impl<S> Platform<S> {
    /// Drops the Host's decks while the session they talk to still runs.
    pub(in crate::host) fn close(platform: &mut Self, _host_id: BeatGridId) {
        platform.decks.close();
    }

    #[cfg(feature = "offline")]
    pub(in crate::host) fn offline() -> Self {
        Self::owner(DeckPass::Blocks(Decks::default()))
    }

    const fn owner(decks: DeckPass) -> Self {
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
        let dispatcher =
            crate::session::native::spawn::<S>(root, view, output_block_frames, output, settings);
        (
            dispatcher,
            Self::owner(DeckPass::Clock(DeckThread::spawn())),
        )
    }
}

impl<S> Host<S>
where
    S: HasPool<f32> + Send + Sync + 'static,
{
    /// Attaches and transfers one fully configured player or decorator into
    /// this Host, then prepares its graph and initial slot before returning.
    /// Audio-device setup may block; musical playback remains stopped.
    ///
    /// # Errors
    /// Returns an error when binding, attachment, or graph preparation fails.
    pub fn insert<P>(&mut self, mut player: P) -> Result<HostOwned<P>, PlayError>
    where
        P: PlayerControlSource<Schema = S>,
    {
        let (grid_id, control) = self.bind_player(&mut player)?;
        self.dispatcher.attach(grid_id)?;
        if let Err(error) = self
            .session
            .platform_mut()
            .decks
            .hold(grid_id, Box::new(player))
        {
            self.dispatcher.detach(grid_id)?;
            return Err(error);
        }
        let owned = self.owned::<P>(grid_id, control);
        if let Err(error) = P::prepare_control(owned.control()) {
            self.remove(&owned)?;
            return Err(error);
        }
        Ok(owned)
    }

    /// Closes the lower runtime on the caller thread, detaches its deck once
    /// graph unregistration has completed, then takes the deck back and drops
    /// it.
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
        self.session
            .platform_mut()
            .decks
            .release(player.id())
            .map(drop)
    }
}
