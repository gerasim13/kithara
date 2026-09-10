#![forbid(unsafe_code)]

use crate::{SlotId, TrackId};

#[derive(Clone, Copy, Debug, Default)]
pub struct ScopeLabel {
    pub deck: Option<SlotId>,
    pub track: Option<TrackId>,
}

impl ScopeLabel {
    #[must_use]
    pub(crate) fn merged_with(self, child: Self) -> Self {
        Self {
            deck: child.deck.or(self.deck),
            track: child.track.or(self.track),
        }
    }
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct EventMeta {
    pub deck: Option<SlotId>,
    pub track: Option<TrackId>,
    pub origin: u64,
    pub seq: u64,
    pub ts_micros: u64,
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Envelope<E> {
    pub event: E,
    pub meta: EventMeta,
}

impl<E> Envelope<E> {
    pub fn map<T, F: FnOnce(E) -> T>(self, f: F) -> Envelope<T> {
        Envelope {
            event: f(self.event),
            meta: self.meta,
        }
    }
}
