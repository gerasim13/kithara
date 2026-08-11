#![forbid(unsafe_code)]

use std::fmt;

use dashmap::mapref::entry::Entry;
use kithara_platform::{CancelToken, sync::Arc};

use super::{PendingResourceSession, WriterClaim, WriterEpoch, WriterIdentity};

/// Elected writer role for one attached consumer.
#[non_exhaustive]
pub struct WriterHandle {
    identity: Arc<WriterIdentity>,
}

impl WriterHandle {
    pub(in crate::index) fn new(
        claim: Arc<WriterClaim>,
        session: &Arc<PendingResourceSession>,
    ) -> Self {
        Self {
            identity: WriterIdentity::new(claim, session),
        }
    }

    /// Clone an epoch capability for fetch callbacks.
    #[must_use]
    pub fn epoch(&self) -> WriterEpoch {
        WriterEpoch::new(Arc::clone(&self.identity))
    }

    /// Whether this handle still owns the current writer epoch.
    #[must_use]
    pub fn is_current(&self) -> bool {
        self.identity
            .session
            .slot
            .state
            .lock()
            .is_current_writer(&self.identity.claim)
    }

    /// Aggregate demand watermark across all live consumers.
    #[must_use]
    pub fn max_watermark(&self) -> u64 {
        self.identity.session.slot.state.lock().max_watermark()
    }

    /// Writer cancellation token.
    #[must_use]
    pub fn writer_cancel(&self) -> CancelToken {
        self.identity.session.slot.writer_cancel.clone()
    }
}

impl Drop for WriterHandle {
    fn drop(&mut self) {
        let session = &self.identity.session;
        let Entry::Occupied(occupied) = session.inner.slots.entry(session.key.clone()) else {
            return;
        };
        if !Arc::ptr_eq(occupied.get(), &session.slot) {
            return;
        }
        let mut state = session.slot.state.lock();
        if state.is_current_writer(&self.identity.claim) {
            state.writer_claim = None;
            let wakers = state.peer_wakers();
            drop(state);
            drop(occupied);
            for waker in wakers {
                waker.wake();
            }
        }
    }
}

impl fmt::Debug for WriterHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WriterHandle")
            .field("key", &self.identity.session.key)
            .finish_non_exhaustive()
    }
}
