#![forbid(unsafe_code)]

mod index;
mod mode;
mod payload;

pub use index::{BoundsError, VariantIndex};
pub use mode::AbrMode;
pub use payload::{
    AbrEvent, AbrProgressSnapshot, AbrReason, BandwidthSource, VariantDuration, VariantInfo,
};
