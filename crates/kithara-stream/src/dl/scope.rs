use kithara_platform::sync::Arc;

use super::{
    downloader::Downloader,
    peer::{Peer, PeerHandle, PeerRef},
};

/// Where a peer registers, and what it fetches on behalf of.
///
/// A [`Downloader`] is the root: a peer registered there fetches for itself.
/// A [`PeerHandle`] is a branch: a peer registered under it is a piece of
/// that peer's download — ranked inside it, crediting its bytes to it, and
/// cancelled with it, because the child's cancel token derives from the
/// branch's.
///
/// The point of the trait is that the two read the same to whoever settles
/// in them. A protocol whose download is carried by several registrations
/// (an HLS variant whose segments are ordinary files) hands its own handle
/// down as the scope, and the piece never learns that it is a piece.
pub trait FetchScope: Send + Sync {
    /// Register `peer` in this scope.
    fn register(&self, peer: Arc<dyn Peer>) -> PeerHandle;

    /// The peer a registration here fetches on behalf of. `None` at the
    /// root, where a peer answers for itself.
    fn parent(&self) -> Option<PeerRef>;
}

impl FetchScope for Downloader {
    fn register(&self, peer: Arc<dyn Peer>) -> PeerHandle {
        // The loop is spawned on the first registration, not at construction:
        // a `Downloader` may be built outside an async context.
        self.ensure_spawned();
        let pool = self.pool();
        let cancel = pool.cancel.clone();
        pool.register_peer(peer, &cancel)
    }

    fn parent(&self) -> Option<PeerRef> {
        None
    }
}

impl FetchScope for PeerHandle {
    fn register(&self, peer: Arc<dyn Peer>) -> PeerHandle {
        // The pool is already running — this handle came out of it.
        self.pool().register_peer(peer, &self.cancel())
    }

    fn parent(&self) -> Option<PeerRef> {
        Some(self.peer_ref())
    }
}
