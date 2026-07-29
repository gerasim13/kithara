#![forbid(unsafe_code)]

#[path = "state/cancel.rs"]
mod cancel_epoch;
#[path = "flow/cas_anchor.rs"]
mod cas_anchor;
mod core;
#[path = "flow/cursor.rs"]
mod cursor;
#[path = "map/descriptor.rs"]
mod descriptor;
#[path = "io/dispatch.rs"]
mod dispatch;
#[path = "flow/evict.rs"]
mod evict;
#[path = "io/fetch.rs"]
mod fetch;
#[path = "io/init.rs"]
mod init;
#[path = "map/layout.rs"]
mod layout;
#[path = "state/lease.rs"]
mod lease;
#[path = "flow/lifecycle.rs"]
mod lifecycle;
#[path = "map/media.rs"]
mod media;
#[path = "map/offsets.rs"]
mod offsets;
#[path = "io/phase.rs"]
mod phase;
#[path = "flow/probe.rs"]
mod probe;
#[path = "flow/queue.rs"]
mod queue;
#[path = "io/read.rs"]
mod read;
#[path = "flow/seek.rs"]
mod seek;
#[path = "flow/seqlock.rs"]
mod seqlock;
#[path = "flow/size.rs"]
mod size;

#[cfg(test)]
pub(crate) use self::core::VariantParts;
#[cfg(test)]
pub(crate) use self::core::test_transport;
#[cfg(test)]
pub(in crate::variant) use self::{core::segment_placeholder_size, probe::SizeDemand};
pub(crate) use self::{
    core::{HlsVariant, PlanCtx, SegmentActivateParams, VariantParams},
    lease::{ReadLease, ReadSession},
    queue::TargetBodyReadiness,
};

#[cfg(test)]
mod tests;
