use super::HlsVariant;
use crate::segment::{FetchClaim, Loaded, PlannedFetch};

impl HlsVariant {
    /// Settle hook: shrinks the appropriate size atom to `actual` and
    /// rebuilds the offset map. Called from
    /// [`FetchSlot::settle`] via `Weak<HlsVariant>::upgrade()` once the
    /// resource commits — for DRM, this is where the post-PKCS7 length
    /// replaces the encrypted estimate.
    ///
    /// Size store and offset recompute happen under the same Layout write
    /// lock — a reader that races in between would see a new size with
    /// stale offsets and fall into a non-existent gap, hanging on
    /// `range_ready`. The closure performs the caller-owned size store and
    /// reports the post-store `init_size` to seed the recompute.
    pub(crate) fn apply_commit(&self, loaded: &FetchClaim<Loaded>) {
        self.layout.apply_commit(&self.segments, || {
            self.apply_loaded_size(loaded.planned(), loaded.final_len());
            self.init_route_size()
        });
        self.complete_exact_seek_if_ready();
    }

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
}
