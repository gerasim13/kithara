use std::task::{Context, Poll};

use bytes::Bytes;
use futures::future::join_all;
use kithara_abr::{Abr, AbrHandle, AbrPeerId};
use kithara_events::{EventBus, RequestPriority};
use kithara_net::{Headers, NetError};
use kithara_platform::{
    CancelGroup, CancelToken,
    sync::{Arc, RwLock},
    time::Instant,
    tokio::sync::{mpsc, oneshot},
};

use super::{cmd::FetchCmd, downloader::DownloaderInner, response::FetchResponse};

/// Channel-path payload: a fully buffered response (headers + body bytes).
///
/// `Send` on every target, unlike a streaming [`FetchResponse`] whose
/// wasm body is `!Send`. The download worker collects the body before
/// replying so the engine worker — possibly a different Web Worker — only
/// ever receives `Send` data. [`PeerHandle::execute`]/[`batch`] re-wrap it
/// into a [`FetchResponse`] so callers see the same API.
pub(super) type CollectedResponse = (Headers, Bytes);

/// Re-wrap a collected channel-path payload into a [`FetchResponse`] so
/// `execute`/`batch` callers keep the streaming API. The body is a single
/// buffered chunk.
fn collected_into_response((headers, bytes): CollectedResponse) -> FetchResponse {
    FetchResponse {
        headers,
        body: bytes.into(),
    }
}

/// Protocol-agnostic contract for download orchestration.
///
/// Protocols (HLS, File) implement this trait. The
/// [`Downloader`](super::Downloader) queries peers through this
/// interface without knowing domain specifics.
///
/// All methods have defaults so that simple peers (File) only need
/// to exist — the Downloader drives everything through `execute()`.
/// Complex peers (HLS) override `poll_next` to let the Downloader
/// drive media segment downloads via per-command `writer`/`on_complete`
/// closures in [`FetchCmd`].
pub trait Peer: Abr {
    /// Yield the next batch of commands for the Downloader to execute.
    ///
    /// Returns `Ready(Some(batch))` with one or more self-contained
    /// [`FetchCmd`]s. Each command carries its own `writer` and
    /// `on_complete` closures — the Downloader calls them directly.
    ///
    /// Returns `Ready(None)` when the peer has no more work (stream
    /// ended). Returns `Pending` when waiting (throttle, idle).
    fn poll_next(&self, _cx: &mut Context<'_>) -> Poll<Option<Vec<FetchCmd>>> {
        Poll::Ready(None)
    }

    /// Whether this peer is active — `High` when what it fetches is wanted
    /// now rather than ahead of need.
    fn priority(&self) -> RequestPriority {
        RequestPriority::LOW
    }

    /// The peer this one fetches on behalf of, when it is not fetching for
    /// itself.
    ///
    /// A composite peer is one logical download carried by several
    /// registrations, each fetching a piece. Declaring the parent keeps the
    /// pieces one download: a child is ranked behind its parent and its
    /// bytes are credited to the parent's identity, so bandwidth accounting
    /// sees one consumer rather than a crowd of anonymous fetches.
    ///
    /// The child's own [`priority`](Self::priority) still orders it against
    /// its siblings — the piece someone is waiting on outranks the pieces
    /// fetched ahead of it.
    fn parent(&self) -> Option<PeerRef> {
        None
    }

    /// Full queue rank of this peer's fetches.
    ///
    /// Composed from [`priority`](Self::priority) and
    /// [`parent`](Self::parent) here, in the trait, so a peer answers for
    /// its own placement instead of leaving the rule to the scheduler: a
    /// root peer's priority stands as-is, and a child's is OR'd into its
    /// parent's one level in ([`RequestPriority::nested`]). Override only
    /// to break that composition.
    fn fetch_priority(&self) -> RequestPriority {
        let own = self.priority();
        // A parent that has gone leaves the child to answer for itself: its
        // own priority stands unnested, since there is no longer an outer
        // rank for it to sit inside.
        self.parent()
            .and_then(|parent| parent.priority())
            .map_or(own, |inherited| inherited | own.nested())
    }

    /// Identity this peer's bytes are credited to — its parent's when it
    /// fetches on another's behalf, otherwise its own (`None`).
    fn credit_to(&self) -> Option<AbrPeerId> {
        self.parent().map(|parent| parent.peer_id())
    }
}

/// Weak reference to a registered peer, naming a [`Peer::parent`]. Weak so
/// a child in flight cannot keep its parent — and through it the whole
/// download — alive.
#[derive(Clone)]
pub struct PeerRef {
    peer: std::sync::Weak<dyn Peer>,
    peer_id: AbrPeerId,
}

impl std::fmt::Debug for PeerRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeerRef")
            .field("peer_id", &self.peer_id)
            .finish_non_exhaustive()
    }
}

impl PeerRef {
    pub(super) fn new(peer: &Arc<dyn Peer>, peer_id: AbrPeerId) -> Self {
        Self {
            peer: Arc::downgrade(peer),
            peer_id,
        }
    }

    /// The ABR identity a child's fetches are credited to.
    pub(super) fn peer_id(&self) -> AbrPeerId {
        self.peer_id
    }

    /// The parent's current priority, or `None` once the parent is gone.
    pub(super) fn priority(&self) -> Option<RequestPriority> {
        Some(self.peer.upgrade()?.priority())
    }
}

/// How the Downloader delivers the response for a command.
pub(super) enum ResponseTarget {
    /// Imperative path: send via oneshot (`execute` / `batch`). Carries a
    /// fully collected [`CollectedResponse`] (not a stream) so the payload
    /// stays `Send` across the download↔engine worker boundary on wasm.
    Channel(oneshot::Sender<Result<CollectedResponse, NetError>>),
    /// Streaming path: per-command `writer`/`on_complete` in [`FetchCmd`].
    Streaming,
}

/// Pair of an [`InternalCmd`] and the **peer-level** cancel token used
/// to discriminate `CancelReason` later in `deliver`.
///
/// The peer-level token lives in [`PeerInner::cancel`] (and the
/// Registry's `PeerEntry::peer_cancel`). Carrying a clone alongside
/// the cmd through the slot queue lets the spawned fetch task report
/// `CancelReason::PeerCancel` without holding a reference back into
/// the Registry.
pub(super) struct SlotEntry {
    pub(super) peer_cancel: CancelToken,
    pub(super) cmd: InternalCmd,
}

/// Per-peer command sent through the channel to the downloader loop.
pub(super) struct InternalCmd {
    /// ABR peer identifier for bandwidth accounting after fetch completes.
    pub(super) peer_id: AbrPeerId,
    pub(super) cancel: CancelGroup,
    pub(super) cmd: FetchCmd,
    /// Wall-clock instant the cmd was placed into a priority slot.
    /// Used to compute `RequestStarted::wait_in_queue` later in the
    /// pipeline.
    pub(super) enqueued_at: Instant,
    /// Bus of the peer that issued this command. Downloader publishes
    /// per-fetch `DownloaderEvent`s here.
    pub(super) bus: Option<EventBus>,
    /// Arena index of the owning peer. `None` when sent from `PeerHandle`
    /// (filled in by Registry on receipt).
    pub(super) peer: Option<thunderdome::Index>,
    /// Stable id allocated by the Downloader on enqueue. Carried in
    /// every `DownloaderEvent` for this fetch's lifecycle so subscribers
    /// can correlate Enqueued → Started → Completed/Failed/Cancelled.
    pub(super) request_id: kithara_events::RequestId,
    pub(super) priority: RequestPriority,
    pub(super) response: ResponseTarget,
}

/// Shared per-peer state. Cancel fires when the last clone is dropped.
struct PeerInner {
    /// ABR side of the double registration. Keeps the peer registered
    /// with the shared `AbrController` until the last `PeerHandle` drops
    /// (the handle's `Drop` calls `controller.unregister`).
    abr: AbrHandle,
    /// Keeps `DownloaderInner` (`HttpClient`, cancel, runtime) alive
    /// for this peer's lifetime.
    _pool: Arc<DownloaderInner>,
    /// Shared with the Registry's `PeerEntry`. Writing through
    /// [`PeerHandle::with_bus`] immediately makes the new bus visible
    /// to both the handle's own imperative path and the Registry's
    /// proactive `poll_next` path.
    bus: Arc<RwLock<Option<EventBus>>>,
    cancel: CancelToken,
    cmd_tx: mpsc::Sender<InternalCmd>,
    /// Reference to the registered peer, vended to children that fetch on
    /// its behalf.
    peer_ref: PeerRef,
}

impl Drop for PeerInner {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Per-peer handle for submitting fetch commands and awaiting
/// responses.
///
/// Cheap to [`Clone`] (one Arc bump). When the last clone is dropped,
/// the peer-level cancel token fires, aborting all in-flight fetches
/// for this peer.
#[derive(Clone)]
pub struct PeerHandle {
    inner: Arc<PeerInner>,
}

impl std::fmt::Debug for PeerHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeerHandle").finish_non_exhaustive()
    }
}

impl PeerHandle {
    pub(super) fn new(
        pool: Arc<DownloaderInner>,
        cancel: CancelToken,
        cmd_tx: mpsc::Sender<InternalCmd>,
        bus: Arc<RwLock<Option<EventBus>>>,
        abr: AbrHandle,
        peer_ref: PeerRef,
    ) -> Self {
        Self {
            inner: Arc::new(PeerInner {
                cancel,
                cmd_tx,
                bus,
                abr,
                peer_ref,
                _pool: pool,
            }),
        }
    }

    /// Reference a child peer uses to declare this one as its
    /// [`Peer::parent`].
    #[must_use]
    pub fn as_parent(&self) -> PeerRef {
        self.inner.peer_ref.clone()
    }

    /// ABR-side handle attached at registration.
    #[must_use]
    pub fn abr(&self) -> &AbrHandle {
        &self.inner.abr
    }

    /// Submit a batch of fetch commands and await all responses.
    ///
    /// Commands execute in parallel. Results are returned **in array
    /// order**, not completion order.
    ///
    /// # Errors
    /// Individual commands may fail independently. Each slot in the
    /// returned `Vec` contains its own `Result`.
    pub async fn batch(&self, cmds: Vec<FetchCmd>) -> Vec<Result<FetchResponse, NetError>> {
        let mut receivers: Vec<Option<oneshot::Receiver<Result<CollectedResponse, NetError>>>> =
            Vec::with_capacity(cmds.len());
        let bus = self.bus();
        let peer_id = self.inner.abr.peer_id();

        for cmd in cmds {
            let (internal, resp_rx) = self.make_imperative(cmd, bus.clone(), peer_id);
            if self.inner.cmd_tx.send(internal).await.is_err() {
                receivers.push(None);
                continue;
            }
            receivers.push(Some(resp_rx));
        }

        join_all(receivers.into_iter().map(|rx| async move {
            match rx {
                Some(resp_rx) => resp_rx
                    .await
                    .unwrap_or(Err(NetError::Cancelled))
                    .map(collected_into_response),
                None => Err(NetError::Cancelled),
            }
        }))
        .await
    }

    /// Currently attached bus, if any.
    #[must_use]
    pub fn bus(&self) -> Option<EventBus> {
        self.inner.bus.read().clone()
    }

    /// Peer-level cancellation token.
    ///
    /// Cancelling this token aborts all in-flight fetches for this
    /// peer. The cancel also fires automatically when the last clone
    /// of this handle is dropped.
    #[must_use]
    pub fn cancel(&self) -> CancelToken {
        self.inner.cancel.clone()
    }

    /// Submit a single fetch command and await the response.
    ///
    /// Always runs at `High` priority — imperative requests are
    /// latency-sensitive.
    ///
    /// # Errors
    /// Returns [`NetError::Cancelled`] when the peer cancel fires,
    /// the downloader shuts down, or the HTTP request itself fails.
    pub async fn execute(&self, cmd: FetchCmd) -> Result<FetchResponse, NetError> {
        let (internal, resp_rx) = self.make_imperative(cmd, self.bus(), self.inner.abr.peer_id());
        self.inner
            .cmd_tx
            .send(internal)
            .await
            .map_err(|_| NetError::Cancelled)?;
        resp_rx
            .await
            .map_err(|_| NetError::Cancelled)?
            .map(collected_into_response)
    }

    /// Build a High-priority imperative `InternalCmd` paired with its
    /// response receiver. Shared by [`Self::execute`] and [`Self::batch`].
    fn make_imperative(
        &self,
        cmd: FetchCmd,
        bus: Option<EventBus>,
        peer_id: AbrPeerId,
    ) -> (
        InternalCmd,
        oneshot::Receiver<Result<CollectedResponse, NetError>>,
    ) {
        let cancel = CancelGroup::new(vec![self.inner.cancel.child()]);
        let (resp_tx, resp_rx) = oneshot::channel();
        let request_id = self.inner._pool.next_request_id();
        let enqueued_at = Instant::now();
        let internal = InternalCmd {
            cmd,
            cancel,
            bus,
            peer_id,
            request_id,
            enqueued_at,
            priority: RequestPriority::HIGH,
            response: ResponseTarget::Channel(resp_tx),
            peer: None,
        };
        (internal, resp_rx)
    }

    /// ABR peer identifier for this handle.
    #[must_use]
    pub fn peer_id(&self) -> AbrPeerId {
        self.inner.abr.peer_id()
    }

    /// Attach an event bus so the Downloader can publish per-peer
    /// [`DownloaderEvent`](kithara_events::DownloaderEvent)s and the ABR
    /// controller can publish [`AbrEvent`](kithara_events::AbrEvent)s to
    /// it. Returns `self` so the call chains naturally after
    /// [`Downloader::register`](super::Downloader::register).
    #[must_use]
    pub fn with_bus(self, bus: EventBus) -> Self {
        *self.inner.bus.write() = Some(bus.clone());
        let _ = self.inner.abr.clone().with_bus(bus);
        self
    }
}
