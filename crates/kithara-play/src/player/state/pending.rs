use kithara_platform::sync::Arc;

use crate::{
    api::TrackId,
    resource::{PreparedGrid, StagingRecipe},
};

/// What the player publishes about the item it plays: its beat grid, its
/// staging recipe and its ABR handle. An armed successor holds it until it
/// becomes the current item.
pub(crate) struct ItemPresentation {
    pub(crate) beat_grid: Arc<PreparedGrid>,
    pub(crate) abr_handle: Option<kithara_abr::AbrHandle>,
    pub(crate) staging: Option<StagingRecipe>,
}

/// Whether the successor is armed behind the current item or already
/// committed as the leading one.
pub(crate) enum PendingNextState {
    /// Attached ahead and not yet current; holds the presentation it
    /// publishes once it becomes current.
    Armed(ItemPresentation),
    /// Committed by a crossfade: the leading item, its presentation published.
    ActivatedReady,
}

impl PendingNextState {
    pub(crate) const fn activated(&self) -> bool {
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

/// The track the processor reported playing, as a handover settles it.
pub(crate) enum Played {
    /// The armed successor, stitched in behind its predecessor, with what it
    /// publishes now that it leads.
    Armed {
        duration_seconds: f64,
        presentation: ItemPresentation,
    },
    /// A withdrawn successor the processor stitched in before it read the
    /// withdrawal.
    Withdrawn,
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
    /// processor holds them until it reports them unloaded and may stitch one
    /// in at any later track end, so a later leading track does not settle
    /// them.
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

    /// The processor unloaded `item_id`, so it can no longer stitch that
    /// withdrawn successor in.
    pub(crate) fn retire(&mut self, item_id: TrackId) {
        self.withdrawn.retain(|withdrawn| *withdrawn != item_id);
        self.uncancelled
            .retain(|uncancelled| *uncancelled != item_id);
    }

    /// The processor played `item_id`: it reported the track's start or its
    /// natural or failed end. That report, not its predecessor's end, makes
    /// the armed successor or a withdrawn one the track that leads; a
    /// withdrawn one leaves the armed successor armed.
    pub(crate) fn settle_played(&mut self, item_id: TrackId) -> Option<Played> {
        let played = match self.next.take() {
            Some(PendingNext {
                state: PendingNextState::Armed(presentation),
                item_id: armed,
                duration_seconds,
                ..
            }) if armed == item_id => Played::Armed {
                duration_seconds,
                presentation,
            },
            next => {
                self.next = next;
                if !(self.withdrawn.contains(&item_id) || self.uncancelled.contains(&item_id)) {
                    return None;
                }
                Played::Withdrawn
            }
        };
        self.withdrawn.clear();
        self.uncancelled
            .retain(|uncancelled| *uncancelled != item_id);
        Some(played)
    }

    /// Retire a committed successor once a track ends. An armed one stays:
    /// only the processor's report that it played makes it lead.
    pub(crate) fn take_activated(&mut self) -> Option<PendingNext> {
        self.next.take_if(|next| next.state.activated())
    }

    /// The playlist dropped the item at `index`: whether that was the
    /// successor, whose index is otherwise shifted down past the gap.
    pub(crate) fn removed_at(&mut self, index: usize) -> bool {
        let Some(next) = self.next.as_mut() else {
            return false;
        };
        if next.index == index {
            return true;
        }
        if next.index > index {
            next.index -= 1;
        }
        false
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
        let presentation = ItemPresentation {
            beat_grid: Arc::default(),
            abr_handle: None,
            staging: None,
        };
        assert!(!PendingNextState::Armed(presentation).activated());
        assert!(PendingNextState::ActivatedReady.activated());
    }
}
