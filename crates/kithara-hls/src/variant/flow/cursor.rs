use std::sync::atomic::Ordering;

use kithara_test_utils::kithara;

use super::{HlsVariant, core::NO_PREFETCH_DEFERRAL};

impl HlsVariant {
    /// Record the cursor byte at which the front-of-queue segment enters the
    /// look-ahead window, or [`NO_PREFETCH_DEFERRAL`] when nothing is
    /// deferred. Written by [`HlsVariant::dispatch`] on every pass, so it
    /// always describes the decision the peer last took.
    pub(super) fn defer_prefetch_until(&self, byte: u64) {
        self.flow.prefetch_resume_at.store(byte, Ordering::Release);
    }

    pub(crate) fn prefetch_anchor(&self) -> u64 {
        self.flow.prefetch_anchor.load(Ordering::Acquire)
    }

    pub(crate) fn register_session_seek(&self, pos: u64, moved: bool) {
        if !self.flow.reader.is_seek_active() {
            self.retire_seek_projection_if_moved(pos);
        }
        if moved {
            self.set_exact_byte_seek_demand(pos);
        }
    }

    #[kithara::probe(variant = self.variant as u64, byte)]
    pub(crate) fn set_prefetch_anchor(&self, byte: u64) {
        self.flow.prefetch_anchor.store(byte, Ordering::Release);
    }

    /// Whether the reader at `consumed` just made the deferred dispatch
    /// decision stale. Consumes the threshold, so it answers `true` exactly once
    /// per deferred segment: the next `dispatch` publishes the threshold for the
    /// segment after it. This is the reader's own progress re-opening a decision
    /// that was taken against an older cursor — no timer re-checks it.
    ///
    /// The caller supplies the position because the two facts have different
    /// owners: the variant plans, so it owns the threshold; the session reads,
    /// so it owns the byte cursor. Reading the variant's own prefetch anchor
    /// instead answers a different question — where the reader is *aimed*, not
    /// how far it has *consumed* — and leaves the peer asleep on real progress.
    pub(crate) fn take_prefetch_resume_at(&self, consumed: u64) -> bool {
        let at = self.flow.prefetch_resume_at.load(Ordering::Acquire);
        at != NO_PREFETCH_DEFERRAL
            && consumed >= at
            && self
                .flow
                .prefetch_resume_at
                .swap(NO_PREFETCH_DEFERRAL, Ordering::AcqRel)
                != NO_PREFETCH_DEFERRAL
    }
}
