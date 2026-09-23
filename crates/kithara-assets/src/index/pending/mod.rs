#![forbid(unsafe_code)]
mod demand;

pub(super) use demand::PendingResourceInner;
pub(crate) use demand::{DemandEntry, PendingResourceIndex};
