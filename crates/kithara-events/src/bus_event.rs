#![forbid(unsafe_code)]

use crate::Event;

#[derive(Clone, Debug, Event)]
#[non_exhaustive]
pub enum BusEvent {
    Overflow { scope: u64, dropped: u64 },
}
