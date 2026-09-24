mod analysis;
mod behavior;
mod catalog;
mod config;
mod report;
mod shape;

pub(crate) use config::run;
pub(super) use config::{Direction, SimilarityConfig, Substitution, TypeRelationConfig};
pub use config::{Profile, SimilarityArgs};
