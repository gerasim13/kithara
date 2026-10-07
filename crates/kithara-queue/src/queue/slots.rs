//! The tracks a queue keeps on its deck's mixer, one slot each.

use kithara_command::Seq;
use kithara_events::TrackId;
use kithara_play::{DeckSnapshot, Slot};

/// What an active track is to the queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Role {
    /// Sounds as the queue's current item.
    Current,
    /// The transition target; `batch` is the deck batch that brings it in,
    /// once sent.
    Incoming { batch: Option<Seq> },
    /// Loaded ahead of the current track's end; not yet a target.
    Preloaded,
    /// Plays its tail out after a transition replaced it.
    Outgoing,
    /// Released; waits for its slot to let it go.
    Leaving,
}

/// One track on the deck's mixer.
pub(super) struct Active<T> {
    pub(super) item: TrackId,
    pub(super) slot: Slot,
    pub(super) track: T,
    pub(super) role: Role,
    /// The dispatcher load the track waits on, until its attach applied.
    pub(super) load: Option<Seq>,
}

/// The active tracks, never more than the mixer has slots; the queue assigns
/// each its slot.
pub(super) struct Slots<T> {
    capacity: u16,
    active: Vec<Active<T>>,
}

impl<T> Slots<T> {
    pub(super) fn new(capacity: u16) -> Self {
        Self {
            capacity,
            active: Vec::with_capacity(usize::from(capacity)),
        }
    }

    /// The lowest slot no active track holds.
    pub(super) fn free_slot(&self) -> Option<Slot> {
        (0..self.capacity)
            .map(Slot::new)
            .find(|slot| self.active.iter().all(|active| active.slot != *slot))
    }

    /// The track to evict for a new one: the quietest by the mixer's last
    /// gain among those `evictable` allows; a fade nearer its end is quieter.
    pub(super) fn quietest(
        &self,
        deck: &DeckSnapshot,
        evictable: impl Fn(&Active<T>) -> bool,
    ) -> Option<usize> {
        self.active
            .iter()
            .enumerate()
            .filter(|(_, active)| evictable(active))
            .min_by(|(_, a), (_, b)| gain(deck, a.slot).total_cmp(&gain(deck, b.slot)))
            .map(|(index, _)| index)
    }

    pub(super) fn push(&mut self, active: Active<T>) {
        self.active.push(active);
    }

    pub(super) fn remove(&mut self, index: usize) -> Active<T> {
        self.active.remove(index)
    }

    pub(super) fn position(&self, find: impl Fn(&Active<T>) -> bool) -> Option<usize> {
        self.active.iter().position(find)
    }

    pub(super) fn get(&self, index: usize) -> Option<&Active<T>> {
        self.active.get(index)
    }

    pub(super) fn get_mut(&mut self, index: usize) -> Option<&mut Active<T>> {
        self.active.get_mut(index)
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = &Active<T>> {
        self.active.iter()
    }

    pub(super) fn iter_mut(&mut self) -> impl Iterator<Item = &mut Active<T>> {
        self.active.iter_mut()
    }

    /// Indices of the tracks `find` picks, in slot-assignment order.
    pub(super) fn indices(&self, find: impl Fn(&Active<T>) -> bool) -> Vec<usize> {
        self.active
            .iter()
            .enumerate()
            .filter(|(_, active)| find(active))
            .map(|(index, _)| index)
            .collect()
    }

    pub(super) fn len(&self) -> usize {
        self.active.len()
    }
}

/// `slot`'s last gain, silent when the mixer has not published it.
fn gain(deck: &DeckSnapshot, slot: Slot) -> f32 {
    deck.slots
        .get(usize::from(slot.get()))
        .map_or(0.0, |slot| slot.gain)
}

#[cfg(test)]
mod tests {
    use kithara_play::SlotSnapshot;
    use kithara_test_utils::kithara;

    use super::*;

    fn active(slot: u16, role: Role) -> Active<()> {
        Active {
            item: TrackId(u64::from(slot)),
            slot: Slot::new(slot),
            track: (),
            role,
            load: None,
        }
    }

    fn deck(gains: &[f32]) -> DeckSnapshot {
        DeckSnapshot {
            slots: gains
                .iter()
                .map(|&gain| SlotSnapshot {
                    gain,
                    ..SlotSnapshot::default()
                })
                .collect(),
            ..DeckSnapshot::default()
        }
    }

    #[kithara::test]
    fn the_lowest_slot_no_track_holds_is_free() {
        let mut slots = Slots::new(3);
        slots.push(active(0, Role::Current));
        slots.push(active(2, Role::Preloaded));

        assert_eq!(slots.free_slot(), Some(Slot::new(1)));
        slots.push(active(1, Role::Outgoing));
        assert_eq!(slots.free_slot(), None);
    }

    #[kithara::test]
    fn the_quietest_evictable_track_is_evicted() {
        let mut slots = Slots::new(3);
        slots.push(active(0, Role::Current));
        slots.push(active(1, Role::Outgoing));
        slots.push(active(2, Role::Leaving));

        let victim = slots.quietest(&deck(&[1.0, 0.25, 0.0]), |active| {
            active.role != Role::Leaving
        });

        assert_eq!(victim, Some(1));
    }
}
