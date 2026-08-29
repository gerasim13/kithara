mod consts;
mod data;
mod endpoints;
mod pages;
mod quality;
pub(crate) mod reads;

pub(crate) use endpoints::{MockRegistry, registry};
pub(crate) use reads::MockReads;
