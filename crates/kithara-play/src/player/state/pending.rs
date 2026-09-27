use kithara_platform::sync::Arc;

use crate::api::TrackId;

/// Whether the armed successor has been activated for the current handover.
///
/// Mirrors the pre-split `PendingNext::activated: bool`:
/// - `Armed` ⇒ `activated == false` (armed, not yet committed).
/// - `ActivatedReady` ⇒ `activated == true` (committed, leading slot).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PendingNextState {
    Armed,
    ActivatedReady,
}

impl PendingNextState {
    pub(crate) const fn activated(self) -> bool {
        matches!(self, Self::ActivatedReady)
    }
}

/// Internal auto-advance state for the next queue item.
///
/// `Playlist` owns the current index; `PendingNext` only tracks the
/// already-enqueued successor and whether it has been activated.
pub(crate) struct PendingNext {
    pub(crate) src: Arc<str>,
    pub(crate) state: PendingNextState,
    pub(crate) item_id: TrackId,
    pub(crate) duration_seconds: f64,
    pub(crate) index: usize,
}

/// Successor loads a handover has handed the processor.
#[derive(Default)]
pub(crate) struct PendingLoads {
    /// The armed or activated successor.
    pub(crate) next: Option<PendingNext>,
    /// Armed successors withdrawn from the processor. The audio thread may
    /// have stitched one in before it read the withdrawal, so they stay in
    /// question until the processor reports the successor it played or a
    /// later leading track replaces them.
    withdrawn: Vec<TrackId>,
    /// Withdrawn successors whose cancel the command ring refused. The
    /// processor still holds them and may stitch one in at any later track
    /// end, so a later leading track does not settle them.
    uncancelled: Vec<TrackId>,
}

impl PendingLoads {
    /// The command ring refused the cancel for `item_id`, so the processor
    /// still holds that preload.
    pub(crate) fn cancel_refused(&mut self, item_id: TrackId) {
        self.withdrawn.retain(|withdrawn| *withdrawn != item_id);
        if !self.uncancelled.contains(&item_id) {
            self.uncancelled.push(item_id);
        }
    }

    const fn in_question(&self) -> bool {
        !(self.withdrawn.is_empty() && self.uncancelled.is_empty())
    }

    /// The processor played `item_id`: it reported the track's start or its
    /// natural end. While a withdrawal is in question that is the stitch that
    /// settles it, when it names the armed successor or a withdrawn one; the
    /// armed successor is consumed only when it is the one played.
    pub(crate) fn settle_played(&mut self, item_id: TrackId) -> bool {
        let armed = self
            .next
            .as_ref()
            .is_some_and(|next| !next.state.activated() && next.item_id == item_id);
        let withdrawn = self.withdrawn.contains(&item_id) || self.uncancelled.contains(&item_id);
        if !self.in_question() || !(armed || withdrawn) {
            return false;
        }
        if armed {
            self.next = None;
        }
        self.withdrawn.clear();
        self.uncancelled
            .retain(|uncancelled| *uncancelled != item_id);
        true
    }

    /// The successor a track's end settles. While a withdrawal is in question
    /// the audio thread may have stitched a withdrawn track in instead of the
    /// armed one, so the armed successor waits for the processor to report the
    /// track it played.
    pub(crate) fn take_at_end(&mut self) -> Option<PendingNext> {
        let activated = self
            .next
            .as_ref()
            .is_some_and(|next| next.state.activated());
        if !self.in_question() || activated {
            self.next.take()
        } else {
            None
        }
    }

    /// Take the successor off the handover. The audio thread may already have
    /// stitched an armed one in, so it stays withdrawn until that is settled.
    pub(crate) fn withdraw(&mut self) -> Option<PendingNext> {
        let pending = self.next.take()?;
        if !pending.state.activated() && !self.withdrawn.contains(&pending.item_id) {
            self.withdrawn.push(pending.item_id);
        }
        Some(pending)
    }

    delegate::delegate! {
        to self.withdrawn {
            /// A new track leads. Every cancel the ring accepted before it
            /// reaches the processor while that track or a newer one leads,
            /// which unloads a withdrawn preload and fades out one already
            /// stitched in, so those withdrawals are settled. A refused cancel
            /// never reached it, so that successor stays in question.
            #[call(clear)]
            pub(crate) fn clear_withdrawn(&mut self);
        }
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test]
    fn pending_next_state_maps_activated_bool() {
        assert!(!PendingNextState::Armed.activated());
        assert!(PendingNextState::ActivatedReady.activated());
    }
}
