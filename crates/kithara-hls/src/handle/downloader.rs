use kithara_assets::AssetScope;
use kithara_bufpool::HasPool;
use kithara_events::EventBus;
use kithara_platform::sync::Arc;
use kithara_stream::dl::{Downloader, Peer, PeerHandle};

/// Single owner of the raw transport + storage + headers quartet and the
/// sole `downloader.register` site. Vends one permanent narrow handle
/// ([`Self::peer_handle`] / [`Self::scope`]) for the
/// loaders that still need full download + disk capability.
pub(crate) struct StreamPeer<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    scope: AssetScope<S>,
    peer_handle: PeerHandle,
}

impl<S> StreamPeer<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    /// Staged-transition distribution accessor (retired S13/S14).
    pub(crate) fn peer_handle(&self) -> PeerHandle {
        self.peer_handle.clone()
    }

    /// Register `peer` on `downloader` and take ownership of the quartet.
    /// The sole `downloader.register(...).with_bus(...)` site.
    pub(crate) fn register(
        downloader: &Downloader,
        peer: Arc<dyn Peer>,
        bus: EventBus,
        scope: AssetScope<S>,
    ) -> Self {
        let peer_handle = downloader.register(peer).with_bus(bus);
        Self { scope, peer_handle }
    }

    /// Staged-transition distribution accessor (retired S13/S14).
    pub(crate) fn scope(&self) -> AssetScope<S> {
        self.scope.clone()
    }
}
