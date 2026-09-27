//! Thresholds of the chain stage.

use serde::Deserialize;

mod consts {
    pub(super) const MIN_UNITS: usize = 4;
    pub(super) const MIN_SIDE_LINES: usize = 40;
    pub(super) const SIMILARITY: f64 = 0.4;
    pub(super) const MIN_TOKENS: usize = 20;
    pub(super) const SHINGLE: usize = 3;
    pub(super) const DEPTH: usize = 6;
    pub(super) const MAX_DYN_TARGETS: usize = 4;
    pub(super) const MAX_VARIANT_HANDLERS: usize = 2;
    pub(super) const ARM_JACCARD: f64 = 0.2;
    pub(super) const ARM_CONTAINMENT: f64 = 0.5;
    pub(super) const DYN_JACCARD: f64 = 0.3;
    pub(super) const DYN_PAIR: f64 = 0.6;
}

/// Thresholds of the chain stage; the `[chains]` table overrides them.
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ChainConfig {
    pub(super) arm_containment: f64,
    /// A decision row needs this Jaccard of its sides, or `arm_containment`.
    pub(super) arm_jaccard: f64,
    /// Impls of one trait method form a row only when their regions reach
    /// this Jaccard, or their best function pair `dyn_pair`.
    pub(super) dyn_jaccard: f64,
    pub(super) dyn_pair: f64,
    /// Jaccard that pairs two roots, or the regions two roots own.
    pub(super) similarity: f64,
    /// Bound of every walk over the call graph.
    pub(super) depth: usize,
    /// A trait-object call with more impls is a hub and links nowhere.
    pub(super) max_dyn_targets: usize,
    /// A variant handled in more functions is a hub and links nowhere.
    pub(super) max_variant_handlers: usize,
    /// Lines on each side.
    pub(super) min_side_lines: usize,
    /// Tokens a function needs before it is compared at all.
    pub(super) min_tokens: usize,
    /// Functions, and arms with code of their own, on both sides together.
    pub(super) min_units: usize,
    /// Tokens per shingle.
    pub(super) shingle: usize,
}

impl Default for ChainConfig {
    fn default() -> Self {
        Self {
            min_units: consts::MIN_UNITS,
            min_side_lines: consts::MIN_SIDE_LINES,
            similarity: consts::SIMILARITY,
            min_tokens: consts::MIN_TOKENS,
            shingle: consts::SHINGLE,
            depth: consts::DEPTH,
            max_dyn_targets: consts::MAX_DYN_TARGETS,
            max_variant_handlers: consts::MAX_VARIANT_HANDLERS,
            arm_jaccard: consts::ARM_JACCARD,
            arm_containment: consts::ARM_CONTAINMENT,
            dyn_jaccard: consts::DYN_JACCARD,
            dyn_pair: consts::DYN_PAIR,
        }
    }
}
