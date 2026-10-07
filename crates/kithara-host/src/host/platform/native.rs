use std::{marker::PhantomData, num::NonZeroU32};

use kithara_bufpool::HasPool;
use kithara_command::Live;
use kithara_platform::sync::{Arc, Mutex};
use kithara_play::{PlayError, player::PlayerControlSource};
use kithara_warp::BeatGridId;

use super::{
    super::{Host, HostOwned},
    PlatformResult,
    deck_pass::{DeckThread, Pace},
    decks::Decks,
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
    decks: DeckThread,
    /// The decks the Host stopped ticking as it closed. They drop with the
    /// platform, after the session that renders them has shut down. Only
    /// `close` reaches them; the lock lets a Host be shared while a deck,
    /// which only its holder reaches, need not be.
    retired: Mutex<Decks>,
    marker: PhantomData<fn() -> S>,
}

impl<S> Platform<S> {
    /// Stops ticking the Host's decks and keeps them until the platform
    /// drops, after the session shutdown that stops their stream.
    pub(in crate::host) fn close(platform: &mut Self, _host_id: BeatGridId) {
        *platform.retired.lock() = platform.decks.close();
    }

    #[cfg(feature = "offline")]
    pub(in crate::host) fn offline() -> Self {
        Self::owner(DeckThread::spawn(Pace::Blocks))
    }

    fn owner(decks: DeckThread) -> Self {
        Self {
            decks,
            retired: Mutex::new(Decks::default()),
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
        (dispatcher, Self::owner(DeckThread::spawn(Pace::Clock)))
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
            .platform_mut()
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
        self.session
            .platform_mut()
            .decks
            .release(player.id())
            .map(drop)
    }
}
