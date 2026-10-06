use kithara_events::TrackId;
use kithara_platform::{CancelToken, sync::Arc};
use kithara_play::Resource;

use crate::error::QueueError;

/// Which loader lane a load attempt occupies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LoadClass {
    /// User-facing selection: one dedicated permit, isolated from
    /// prefetch so a hung background lane cannot starve selection.
    Interactive,
    /// Append-time background prefetch, capped by
    /// [`QueueConfig::max_concurrent_loads`](crate::QueueConfig::max_concurrent_loads).
    Prefetch,
}

/// Claim ticket held by a spawned attempt task. Lifecycle reports are
/// generation-checked, so a replaced ticket silently loses its claim.
#[derive(Clone, Copy)]
pub(crate) struct Ticket {
    pub(crate) id: TrackId,
    pub(crate) generation: u64,
}

/// What a load attempt reports to the queue that owns its track. The queue
/// applies each report in the order its attempt posted them, and only while
/// that attempt is still the track's live one.
pub(crate) enum AttemptReport {
    /// The attempt won its lane permit and started loading.
    Started(Ticket),
    /// The downloader found the attempt's transfer slow.
    Slow(Ticket),
    /// The attempt failed on a cause a later ask can answer, and asks again.
    /// Whether anyone still waits for it is the queue's call, made against
    /// its selection as it stands: an attempt nobody selected ends, its
    /// track failed with `error`.
    Retrying { ticket: Ticket, error: QueueError },
    /// The attempt read its track's cover. `attempt` is the attempt's token,
    /// which outlives the attempt in the resource it built: the audio never
    /// waits for the cover.
    Cover {
        id: TrackId,
        attempt: CancelToken,
        cover: Arc<Vec<u8>>,
    },
    /// The attempt ended with a resource, a failure, or a cancel.
    Finished {
        ticket: Ticket,
        outcome: Result<Box<Resource>, QueueError>,
    },
}

/// A track's live load attempt. Dropping the guard armed cancels the
/// attempt's per-track token, so removing a track aborts the load without an explicit call.
pub(crate) struct AttemptGuard {
    /// The user's selection wants this track. Set when the track is selected,
    /// which can happen while a background prefetch attempt is already running:
    /// the lane the attempt was spawned into is fixed, being wanted is not.
    pub(crate) selected: bool,
    pub(crate) waiting: bool,
    pub(crate) generation: u64,
    /// `None` = disarmed: the token now belongs to the built `Resource`.
    cancel: Option<CancelToken>,
}

impl AttemptGuard {
    pub(crate) const fn new(generation: u64, cancel: CancelToken) -> Self {
        Self {
            generation,
            waiting: true,
            cancel: Some(cancel),
            selected: false,
        }
    }

    /// Give the token up to its next owner; dropping then cancels nothing.
    pub(crate) fn disarm(&mut self) {
        self.cancel = None;
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancel.as_ref().is_none_or(CancelToken::is_cancelled)
    }
}

impl Drop for AttemptGuard {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test]
    fn drop_cancels_armed_guard() {
        let token = CancelToken::never().child();
        drop(AttemptGuard::new(0, token.clone()));
        assert!(token.is_cancelled());
    }

    #[kithara::test]
    fn drop_after_disarm_cancels_nothing() {
        let token = CancelToken::never().child();
        let mut guard = AttemptGuard::new(0, token.clone());
        guard.disarm();
        drop(guard);
        assert!(!token.is_cancelled());
    }
}
