#![forbid(unsafe_code)]

use std::num::NonZeroU64;

use kithara_net::NetError;
use kithara_platform::time::Duration;
use url::Url;

/// Stable id for a single Downloader request.
///
/// Allocated internally by the Downloader's `Registry` when wrapping a
/// `FetchCmd` into an `InternalCmd`. Echoed in every
/// [`DownloaderEvent`] for the same logical fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RequestId(NonZeroU64);

impl RequestId {
    /// Construct from a non-zero `u64`. Use a monotonic source (e.g.
    /// an `AtomicU64` started at 1).
    #[must_use]
    pub const fn new(id: NonZeroU64) -> Self {
        Self(id)
    }

    /// Get the inner `u64` for logging.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// HTTP method of a Downloader request.
///
/// Lives in `kithara-events` (not `kithara-stream`) because both the
/// command type and the lifecycle events refer to it; keeping it next
/// to the events avoids the dependency cycle.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum RequestMethod {
    /// HTTP GET, streaming body. Default — used for large downloads
    /// (segments, files) that write directly to storage.
    #[default]
    Get,
    /// HTTP HEAD, headers only. Used for metadata queries
    /// (`Content-Length`).
    Head,
}

bitflags::bitflags! {
    /// Where a request sits in the download queue: one field, one bit per
    /// reason to go first.
    ///
    /// Bits compose with `|`, so a request carried by a composite peer takes
    /// its parent's bits plus its own, and the queue drains from the highest
    /// bits down — setting a bit only ever moves a request forward. Empty is
    /// the background rank: prefetch and idle downloads.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct RequestPriority: u8 {
        /// The peer these bytes are for is active.
        const PEER = 0b10;
        /// A consumer is waiting on this request, as opposed to it being
        /// fetched ahead of need.
        const REQUEST = 0b01;

        /// Latency-sensitive: what a peer sets when it is the active one.
        const HIGH = Self::PEER.bits();
        /// Background: prefetch, idle downloads. The default rank.
        const LOW = 0;
    }
}

impl RequestPriority {
    /// Number of distinct ranks — every combination of the bits.
    #[must_use]
    pub const fn rank_count() -> usize {
        1 << 2
    }

    /// This priority as a child's contribution to its parent's.
    ///
    /// What ranks a peer among its own consumers ranks its pieces among
    /// each other, one level in: a composite peer's parent already carries
    /// the outer bit, so the child spends the inner one.
    #[must_use]
    pub const fn nested(self) -> Self {
        Self::from_bits_truncate(self.bits() >> 1)
    }

    /// Queue slot: rank 0 drains first, so the highest bits map to the
    /// lowest index.
    #[must_use]
    pub const fn slot(self) -> usize {
        (Self::all().bits() - self.bits()) as usize
    }
}

/// Why a fetch was cancelled.
///
/// Distinguishes the cancel paths so subscribers can tell e.g. a
/// seek-driven epoch flush from a peer drop or a downloader-wide
/// shutdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelReason {
    /// The protocol's epoch cancel token fired (e.g. HLS bumped
    /// `seek_epoch`, invalidating in-flight fetches of the prior
    /// epoch).
    EpochCancel,
    /// The peer's own cancel token fired — the last `PeerHandle` clone
    /// was dropped, the protocol is shutting down its track.
    PeerCancel,
    /// Downloader-wide shutdown (the `Downloader` cancel token fired).
    DownloaderShutdown,
    /// The request's `CancelGroup` was already cancelled when the
    /// Downloader tried to spawn the fetch — the fetch never started.
    BeforeStart,
}

/// Events emitted by the unified downloader layer.
///
/// Published on the **peer's bus scope**, set via
/// `PeerHandle::with_bus`. A per-track subscriber sees only its own
/// fetches; a root-bus subscriber sees fetches from every peer.
///
/// Every variant for a single fetch carries the same [`RequestId`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum DownloaderEvent {
    /// Request was accepted by the Downloader and placed into a
    /// priority slot. Published exactly once when `Registry::poll_peers`
    /// pushes the wrapped command into `slots[idx]`. Carries everything
    /// a subscriber needs to build a `request_id → meaning` table.
    RequestEnqueued {
        request_id: RequestId,
        url: Url,
        method: RequestMethod,
        priority: RequestPriority,
    },
    /// HTTP fetch started — slot acquired, task spawned. Between
    /// [`RequestEnqueued`](Self::RequestEnqueued) and this event there
    /// can be an arbitrary delay bounded by `max_concurrent` (slot
    /// pressure indicator: `wait_in_queue`).
    RequestStarted {
        request_id: RequestId,
        /// Time from `RequestEnqueued` to here.
        wait_in_queue: Duration,
    },
    /// `DownloaderConfig::soft_timeout` elapsed without the fetch
    /// completing. Informational; the request keeps running.
    LoadSlow {
        request_id: RequestId,
        elapsed: Duration,
    },
    /// HTTP body finished successfully.
    RequestCompleted {
        request_id: RequestId,
        bytes_transferred: u64,
        /// Total wall time from `RequestStarted` to here.
        duration: Duration,
        /// Pre-computed (`bytes / duration` → bps) so subscribers
        /// don't repeat the math.
        bandwidth_bps: u64,
    },
    RequestRetrying {
        request_id: RequestId,
        attempt: u32,
        max_retries: u32,
        error: NetError,
        backoff: Duration,
    },
    BodyStalled {
        request_id: RequestId,
        consumed: u64,
        expected: Option<u64>,
        stall: Duration,
    },
    BodyResumed {
        request_id: RequestId,
        resume_number: u32,
        from_offset: u64,
        honoured_range: bool,
    },
    RetryExhausted {
        request_id: RequestId,
        max_retries: u32,
        consumed: u64,
        error: NetError,
    },
    FirstByte {
        request_id: RequestId,
        ttfb: Duration,
        status: u16,
        partial: bool,
    },
    /// HTTP fetch ended with a network-level error.
    RequestFailed {
        request_id: RequestId,
        error: NetError,
        /// `error.retryability() == Retryability::Transient` — pre-evaluated.
        retryable: bool,
    },
    /// HTTP fetch was cancelled before completion.
    RequestCancelled {
        request_id: RequestId,
        reason: CancelReason,
        /// Bytes received before the cancel fired (if any).
        bytes_transferred: u64,
    },
    /// Effective priority of an in-queue (not-yet-started) request
    /// changed. Reserved shape — the Downloader does not emit this
    /// today (priority is immutable post-enqueue). Will be emitted
    /// when the scheduler learns to demote prefetch on demand arrival.
    PriorityChanged {
        request_id: RequestId,
        from: RequestPriority,
        to: RequestPriority,
    },
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::RequestPriority;

    /// Every combination is its own queue, and more reasons to go first
    /// means an earlier one. This is the whole ordering contract: the
    /// scheduler indexes its queues by [`RequestPriority::slot`] and drains
    /// them in index order.
    #[kithara::test]
    fn more_reasons_to_hurry_means_an_earlier_queue() {
        let ranks = [
            RequestPriority::PEER | RequestPriority::REQUEST,
            RequestPriority::PEER,
            RequestPriority::REQUEST,
            RequestPriority::empty(),
        ];
        let slots: Vec<usize> = ranks.iter().map(|rank| rank.slot()).collect();

        assert_eq!(slots, vec![0, 1, 2, 3]);
        assert!(
            slots
                .iter()
                .max()
                .is_some_and(|max| *max < RequestPriority::rank_count()),
            "every rank must address a queue that exists"
        );
    }

    /// The aliases the call sites use are the bits, not a parallel scale.
    #[kithara::test]
    fn the_high_low_aliases_are_the_bits() {
        assert_eq!(RequestPriority::HIGH, RequestPriority::PEER);
        assert_eq!(RequestPriority::LOW, RequestPriority::empty());
        assert_eq!(RequestPriority::default(), RequestPriority::LOW);
    }

    /// Composition: a child's urgency ranks it among its siblings, one
    /// level below whatever its parent contributes — so a parent's rank is
    /// never displaced by a child's.
    #[kithara::test]
    fn nesting_demotes_one_level() {
        assert_eq!(RequestPriority::PEER.nested(), RequestPriority::REQUEST);
        assert_eq!(RequestPriority::REQUEST.nested(), RequestPriority::empty());
        assert_eq!(RequestPriority::empty().nested(), RequestPriority::empty());
        assert_eq!(
            RequestPriority::PEER | RequestPriority::PEER.nested(),
            RequestPriority::PEER | RequestPriority::REQUEST,
            "an active peer's own urgent request takes the first queue"
        );
    }
}
