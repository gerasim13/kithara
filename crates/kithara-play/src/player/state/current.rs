use kithara_events::{EventBus, TrackId};
use kithara_platform::sync::Mutex;

use crate::api::PlayerEvent;

/// The item the deck leads, as the player last announced it.
pub(crate) struct CurrentItem {
    bus: EventBus,
    item: Mutex<Option<TrackId>>,
}

impl CurrentItem {
    pub(crate) fn new(bus: EventBus) -> Self {
        Self {
            bus,
            item: Mutex::default(),
        }
    }

    /// Sole publisher of `CurrentItemChanged`: emits only when `item` differs
    /// from the last announced item, so a `play()` resume of the same item
    /// stays quiet.
    pub(crate) fn announce(&self, item: TrackId) {
        let previous = self.item.lock().replace(item);
        if previous != Some(item) {
            self.bus
                .publish(PlayerEvent::CurrentItemChanged { item: Some(item) });
        }
    }

    /// The deck leads nothing any more; the next announcement is news.
    pub(crate) fn clear(&self) {
        *self.item.lock() = None;
    }

    pub(crate) fn get(&self) -> Option<TrackId> {
        *self.item.lock()
    }
}

#[cfg(test)]
mod tests {
    use kithara_events::Envelope;
    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test(native)]
    fn an_item_is_announced_once_until_another_or_a_clear() {
        let bus = EventBus::default();
        let mut events = bus.subscribe();
        let current = CurrentItem::new(bus);
        let first = TrackId::from(7_u64);
        let second = TrackId::from(8_u64);

        current.announce(first);
        current.announce(first);
        current.announce(second);
        current.clear();
        current.announce(second);

        let announced: Vec<_> = std::iter::from_fn(|| events.try_recv().ok())
            .map(|Envelope { event, .. }: Envelope<PlayerEvent>| event)
            .collect();
        assert!(
            matches!(
                announced.as_slice(),
                [
                    PlayerEvent::CurrentItemChanged { item: Some(a) },
                    PlayerEvent::CurrentItemChanged { item: Some(b) },
                    PlayerEvent::CurrentItemChanged { item: Some(c) },
                ] if *a == first && *b == second && *c == second
            ),
            "{announced:?}"
        );
        assert_eq!(current.get(), Some(second));
    }
}
