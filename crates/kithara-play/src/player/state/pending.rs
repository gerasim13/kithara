use kithara_platform::sync::Arc;
use kithara_sync::LoadGeneration;

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
    pub(crate) load: LoadGeneration,
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
    withdrawn: Vec<(TrackId, LoadGeneration)>,
    /// Withdrawn successors whose cancel the command ring refused. The
    /// processor holds them until it reports them unloaded and may stitch one
    /// in at any later track end, so a later leading track does not settle
    /// them.
    uncancelled: Vec<(TrackId, LoadGeneration)>,
}

impl PendingLoads {
    /// The command ring refused the cancel for `refused`, so the processor
    /// still holds that preload.
    pub(crate) fn cancel_refused(&mut self, refused: (TrackId, LoadGeneration)) {
        self.withdrawn
            .retain(|(withdrawn, _)| *withdrawn != refused.0);
        if !self.uncancelled.contains(&refused) {
            self.uncancelled.push(refused);
        }
    }

    const fn in_question(&self) -> bool {
        !(self.withdrawn.is_empty() && self.uncancelled.is_empty())
    }

    /// The processor unloaded `item_id`, so it can no longer stitch that
    /// withdrawn successor in.
    pub(crate) fn retire(&mut self, item_id: TrackId) {
        self.withdrawn
            .retain(|(withdrawn, _)| *withdrawn != item_id);
        self.uncancelled
            .retain(|(uncancelled, _)| *uncancelled != item_id);
    }

    /// The processor played `item_id`: it reported the track's start or its
    /// natural or failed end. While a withdrawal is in question that is the
    /// stitch that settles it, when it names the armed successor or a
    /// withdrawn one, and the load the processor played is returned; the armed
    /// successor is consumed only when it is the one played.
    pub(crate) fn settle_played(&mut self, item_id: TrackId) -> Option<LoadGeneration> {
        let armed = self
            .next
            .as_ref()
            .filter(|next| !next.state.activated() && next.item_id == item_id)
            .map(|next| next.load);
        let withdrawn = self
            .withdrawn
            .iter()
            .chain(&self.uncancelled)
            .find(|(id, _)| *id == item_id)
            .map(|(_, load)| *load);
        if !self.in_question() {
            return None;
        }
        let played = armed.or(withdrawn)?;
        if armed.is_some() {
            self.next = None;
        }
        self.withdrawn.clear();
        self.uncancelled
            .retain(|(uncancelled, _)| *uncancelled != item_id);
        Some(played)
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
        let load = (pending.item_id, pending.load);
        if !pending.state.activated() && !self.withdrawn.contains(&load) {
            self.withdrawn.push(load);
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
