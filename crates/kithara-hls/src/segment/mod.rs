pub(crate) mod core;
pub(crate) mod fetch;
pub(crate) mod resolve;
pub(crate) mod size;
pub(crate) mod state;

pub(crate) use core::{
    FetchCell, InitSegment, MediaSegment, Segment, SegmentContent, SegmentFile, SegmentSource,
};

pub(crate) use fetch::{FetchClaim, PlannedFetch, SegmentSettle};
pub(crate) use size::SegmentSize;
pub(crate) use state::{Downloading, Loaded, SegmentSlotState};
