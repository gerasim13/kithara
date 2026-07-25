pub(crate) mod acquire;
pub(crate) mod core;
pub(crate) mod fetch;
pub(crate) mod resolve;
pub(crate) mod size;
pub(crate) mod state;

pub(crate) use core::{InitSegment, MediaSegment, Segment, SegmentContent, SegmentSource};

pub(crate) use fetch::{FetchClaim, FetchSlot, PlannedFetch};
pub(crate) use size::SegmentSize;
pub(crate) use state::{Downloading, Loaded, SegmentSlotState};
