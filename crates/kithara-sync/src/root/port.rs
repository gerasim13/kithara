use kithara_warp::BeatGridId;

use crate::{GroupState, SyncGroup, SyncReceiptInbox};

/// Where one slot's audio receipts wait for the owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InboxAt {
    /// The `slot`-th live slot of the `deck`-th deck.
    Live { deck: usize, slot: usize },
    /// The `index`-th slot that has left the graph while the audio callback
    /// may still hold its processor.
    Retiring(usize),
}

/// The session facts the root reads and the slot inboxes it drains, which
/// the Host holds beside it.
pub trait RootPort<G: SyncGroup<NestedGroup = G>> {
    /// How many decks the session graph projects.
    fn decks(&self) -> usize;

    /// How many live slots the `deck`-th deck has.
    fn slots(&self, deck: usize) -> usize;

    /// How many slots are retiring.
    fn retiring_len(&self) -> usize;

    /// The inbox at `at`, if the Host holds one there.
    fn inbox(&mut self, at: InboxAt) -> Option<&mut SyncReceiptInbox>;

    /// Destroys the `index`-th retiring slot, which is below
    /// [`Self::retiring_len`], and returns the deck group it played in.
    fn remove_retiring(&mut self, index: usize) -> BeatGridId;

    /// Whether the deck `group` is still projected with no slot live or
    /// retiring.
    fn is_quiesced(&self, group: BeatGridId) -> bool;

    /// Whether the session graph projects the grid `id`.
    fn is_projected(&self, id: BeatGridId) -> bool;

    /// Makes `root` what the session's readers see.
    fn publish(&self, root: &GroupState<G>);
}
