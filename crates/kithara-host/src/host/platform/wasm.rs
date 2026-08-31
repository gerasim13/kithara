use std::{collections::HashMap, mem};

use kithara_platform::sync::{Arc, Mutex};
use kithara_play::{
    PlayError,
    player::{Player, PlayerControlSource, PlayerMember},
};
use kithara_warp::{
    BeatGridId, SyncAdmission, SyncCapability, SyncError, SyncOperation, SyncRejected,
};

use super::super::{Host, HostConfig, HostOwned, SessionRoot};
use crate::{
    session::{HostDispatcher, RootView},
    wasm::HostRoute,
};

pub(in crate::host) struct Platform {
    remote_routes: Mutex<Vec<Arc<HostRoute>>>,
    remote_residents: Option<HashMap<BeatGridId, Box<dyn Player>>>,
}

impl Platform {
    pub(in crate::host) fn owner() -> Self {
        Self {
            remote_routes: Mutex::default(),
            remote_residents: None,
        }
    }

    fn remote() -> Self {
        Self {
            remote_routes: Mutex::default(),
            remote_residents: Some(HashMap::new()),
        }
    }

    fn require_remote(&self) -> Result<(), PlayError> {
        if self.remote_residents.is_some() {
            return Ok(());
        }
        Err(PlayError::Internal(
            "wasm players must be inserted from their owning Worker".into(),
        ))
    }

    fn insert_resident<P>(
        &mut self,
        id: BeatGridId,
        player: P,
    ) -> Result<Option<Box<dyn Player>>, PlayError>
    where
        P: Player,
    {
        self.remote_residents
            .as_mut()
            .ok_or_else(|| {
                PlayError::Internal("wasm Worker resident registry is unavailable".into())
            })
            .map(|residents| residents.insert(id, Box::new(player)))
    }

    fn close_resident(&mut self, id: BeatGridId) -> Result<(), PlayError> {
        self.remote_residents
            .as_mut()
            .and_then(|residents| residents.get_mut(&id))
            .ok_or_else(|| PlayError::Internal("attached wasm player lost its owner".into()))?
            .close()
    }

    fn release_resident(&mut self, id: BeatGridId) -> Result<(), PlayError> {
        let resident = self
            .remote_residents
            .as_mut()
            .and_then(|residents| residents.remove(&id))
            .ok_or_else(|| {
                PlayError::Internal("detached wasm player lost its Worker resident".into())
            })?;
        drop(resident);
        Ok(())
    }

    fn release_on_session_gone<T>(
        &mut self,
        id: BeatGridId,
        result: Result<T, PlayError>,
    ) -> Result<T, PlayError> {
        match result {
            Err(error @ PlayError::SessionGone { .. }) => {
                self.release_resident(id)?;
                Err(error)
            }
            result => result,
        }
    }

    pub(in crate::host) fn close(platform: &mut Self, host_id: BeatGridId) {
        for route in mem::take(&mut *platform.remote_routes.lock()) {
            route.close();
        }
        if let Some(residents) = platform.remote_residents.take()
            && !residents.is_empty()
        {
            let resident_count = residents.len();
            for resident in residents.into_values() {
                mem::forget(resident);
            }
            tracing::error!(
                ?host_id,
                resident_count,
                "remote wasm Host dropped before its players detached; retaining residents"
            );
        }
    }

    pub(in crate::host) fn transact(
        dispatcher: &Arc<dyn HostDispatcher>,
        operation: SyncOperation<PlayerMember>,
    ) -> Result<SyncAdmission, SyncRejected<PlayerMember>> {
        if matches!(&operation, SyncOperation::Topology { .. }) {
            return Err(SyncRejected::new(
                SyncError::CapabilityUnavailable {
                    capability: SyncCapability::Topology,
                },
                operation,
            ));
        }
        dispatcher.transact(operation)
    }
}

impl Host {
    /// Creates the platform session and its canonical synchronization root.
    ///
    /// # Errors
    /// Returns an error when a canonical grid identity cannot be allocated.
    pub fn new(config: HostConfig) -> Result<Self, PlayError> {
        let SessionRoot {
            id,
            sample_rate,
            group,
            view,
        } = Self::session_root(config)?;
        let dispatcher = crate::session::web::spawn(group, view.clone(), sample_rate)?;
        Ok(Self::owner(id, view, dispatcher))
    }

    pub(crate) fn remote(
        id: BeatGridId,
        root_view: RootView,
        dispatcher: Arc<dyn HostDispatcher>,
    ) -> Self {
        Self {
            id,
            owns_session: false,
            root_view,
            dispatcher,
            platform: Platform::remote(),
        }
    }

    pub(crate) fn remote_identity(&self) -> (BeatGridId, RootView) {
        (self.id, self.root_view.clone())
    }

    pub(crate) fn register_remote_route(&self, route: Arc<HostRoute>) {
        self.platform.remote_routes.lock().push(route);
    }

    /// Attaches and transfers one fully configured player or decorator into
    /// this Host before it can register its lower graph projection.
    ///
    /// # Errors
    /// Returns an error when session binding or canonical attachment fails.
    pub fn insert<P>(&mut self, mut player: P) -> Result<HostOwned<P>, PlayError>
    where
        P: PlayerControlSource,
    {
        self.platform.require_remote()?;
        let (grid_id, control) = self.bind_player(&mut player)?;
        let member = player.take_host_member()?;
        self.attach_member(member)?;
        if let Some(replaced) = self.platform.insert_resident(grid_id, player)? {
            mem::forget(replaced);
            return Err(PlayError::Internal(
                "wasm player residence changed during insertion".into(),
            ));
        }
        Ok(self.owned(grid_id, control))
    }

    /// Closes the lower runtime on the caller thread, then detaches its
    /// canonical member after graph unregistration has completed.
    ///
    /// # Errors
    /// Returns an error when close or canonical detachment fails.
    pub fn remove<P>(&mut self, player: &HostOwned<P>) -> Result<(), PlayError>
    where
        P: PlayerControlSource,
    {
        self.validate_removal(player)?;
        let id = player.id();
        let close_result = self.platform.close_resident(id);
        self.platform.release_on_session_gone(id, close_result)?;
        let detach_result = self.detach_member(id);
        self.platform.release_on_session_gone(id, detach_result)?;
        self.platform.release_resident(id)
    }
}

#[cfg(test)]
#[path = "wasm_tests.rs"]
mod tests;
