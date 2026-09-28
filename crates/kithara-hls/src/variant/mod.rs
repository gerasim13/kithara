#![forbid(unsafe_code)]

mod core;
mod flow;
mod io;
mod map;
mod profile;
mod reader_runtime;

#[cfg(test)]
pub(crate) use self::core::VariantParts;
#[cfg(test)]
pub(in crate::variant) use self::{core::segment_placeholder_size, flow::probe::SizeDemand};
pub(crate) use self::{
    core::{HlsVariant, PlanConfig, PlanCtx},
    flow::{plan_queue::PlanRevision, seek::ResolvedSeekProjection},
    io::dispatch::DispatchTokens,
    profile::VariantReaderPreparation,
};

#[cfg(test)]
mod tests;
