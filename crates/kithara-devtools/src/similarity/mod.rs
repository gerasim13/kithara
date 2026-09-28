mod analysis;
mod behavior;
mod catalog;
mod chains;
mod config;
mod report;
mod shape;

pub(crate) use config::run;
pub(super) use config::{Direction, SimilarityConfig, Substitution, TypeRelationConfig};
pub use config::{Profile, SimilarityArgs};
