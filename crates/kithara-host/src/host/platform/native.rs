use std::marker::PhantomData;

use kithara_bufpool::HasPool;
use kithara_platform::sync::Arc;
use kithara_play::{
    GroupState, PlayError,
    player::{PlayerControlSource, PlayerMember},
};
use kithara_warp::{BeatGridId, SyncAdmission, SyncOperation, SyncRejected};

use super::super::{Host, HostOwned, PlatformResult};
use crate::session::{HostDispatcher, RootView};

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
    marker: PhantomData<fn() -> S>,
}

impl<S> Platform<S> {
    pub(in crate::host) const fn owner() -> Self {
        Self {
            marker: PhantomData,
        }
    }

    pub(in crate::host) const fn close(_platform: &mut Self, _host_id: BeatGridId) {}

    pub(in crate::host) fn transact(
        _platform: &Self,
        dispatcher: &Arc<dyn HostDispatcher<S>>,
        operation: SyncOperation<PlayerMember>,
    ) -> Result<SyncAdmission, SyncRejected<PlayerMember>> {
        dispatcher.transact(operation)
    }

    pub(in crate::host) fn realtime(
        group: GroupState<PlayerMember>,
        view: RootView,
        sample_rate: std::num::NonZeroU32,
    ) -> StartedPlatform<S>
    where
        S: HasPool<f32> + Send + Sync + 'static,
    {
        let dispatcher = crate::session::native::spawn::<S>(group, view, sample_rate);
        (dispatcher, Self::owner())
    }

    #[cfg(feature = "offline")]
    pub(in crate::host) const fn offline() -> Self {
        Self::owner()
    }
}

impl<S> Host<S>
where
    S: HasPool<f32> + Send + Sync + 'static,
{
    /// Attaches and transfers one fully configured player or decorator into
    /// this Host before it can register its lower graph projection.
    ///
    /// # Errors
    /// Returns an error when session binding or canonical attachment fails.
    pub fn insert<P>(&mut self, mut player: P) -> Result<HostOwned<P>, PlayError>
    where
        P: PlayerControlSource<Schema = S>,
    {
        let (grid_id, control) = self.bind_player(&mut player)?;
        self.attach_member(PlayerMember::new(player))?;
        Ok(self.owned(grid_id, control))
    }

    /// Closes the lower runtime on the caller thread, then detaches its
    /// canonical member after graph unregistration has completed.
    ///
    /// # Errors
    /// Returns an error when close or canonical detachment fails.
    pub fn remove<P>(&mut self, player: &HostOwned<P>) -> Result<(), PlayError>
    where
        P: PlayerControlSource<Schema = S>,
    {
        self.validate_removal(player)?;
        P::close_control(player.control())?;
        self.detach_member(player.id())
    }
}
