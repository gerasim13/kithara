use kithara_platform::CancelToken;

use super::{HlsVariant, ReadLease};
use crate::segment::{FetchClaim, Loaded, PlannedFetch};

impl ReadLease {
    /// Settle hook: shrinks the appropriate size atom to `actual`, rebuilds
    /// the offset map, and follows the shift through this lease's pending
    /// exact-seek demand. Called from [`FetchSlot::settle`] via
    /// `Weak<ReadLease>::upgrade()` on the lease that claimed the slot, once
    /// the resource commits — for DRM, this is where the post-PKCS7 length
    /// replaces the encrypted estimate.
    ///
    /// Size store and offset recompute happen under the same Layout write
    /// lock — a reader that races in between would see a new size with
    /// stale offsets and fall into a non-existent gap, hanging on
    /// `range_ready`. The closure performs the caller-owned size store and
    /// reports the post-store `init_size` to seed the recompute.
    pub(crate) fn apply_commit(&self, loaded: &FetchClaim<Loaded>) {
        self.variant
            .layout
            .apply_commit(&self.variant.segments, || {
                self.variant
                    .apply_loaded_size(loaded.planned(), loaded.final_len());
                self.variant.init_route_size()
            });
        self.complete_exact_seek_if_ready();
    }
}

impl HlsVariant {
    /// Settle-side size store: shrink the appropriate atom to `final_len`.
    /// The caller runs this inside [`Layout::apply_commit`](
    /// offsets::Layout::apply_commit)'s write-lock so a reader never
    /// observes a new size against a stale offset table.
    pub(super) fn apply_loaded_size(&self, planned: PlannedFetch, final_len: u64) {
        match planned {
            PlannedFetch::Init => {
                // Only a `Some(Init)` slot is ever settled (it is the only init
                // that gets fetched). A `None` init has no size atom; a stray
                // settle is a no-op rather than resurrecting an init.
                if let Some(init) = self.segments.init.as_ref() {
                    init.set_loaded_size(final_len);
                }
            }
            PlannedFetch::Segment(idx) => {
                if let Some(slot) = self.segments.get(idx as usize) {
                    slot.set_loaded_size(final_len);
                }
            }
        }
    }

    delegate::delegate! {
        to self.cancel_epoch {
            pub(crate) fn cancel(&self);
            #[call(handle)]
            pub(crate) fn cancel_handle(&self) -> CancelToken;
            /// Replace the cancel token with a fresh child of `master_cancel`.
            /// Called on every re-activation path ([`Self::reset_to_full_range`]
            /// and [`Self::activate_at_segment_with_shift`]) so a variant that
            /// was deactivated (cancelled) on a prior ABR commit can dispatch
            /// fetches again. Without this, the second activation of the same
            /// `HlsVariant` instance enqueues `FetchCmd`s under a permanently
            /// cancelled token — every fetch settles immediately as
            /// `stale (cancelled)` and the reader hangs on `wait_range`.
            /// In-flight clones held by prior fetches stay cancelled (correct —
            /// they belong to the previous epoch and must not write).
            #[call(rearm)]
            pub(crate) fn rearm_cancel(&self);
        }
    }
}
