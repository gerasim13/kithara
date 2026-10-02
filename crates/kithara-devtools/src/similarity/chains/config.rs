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
#[derive(Clone, Debug, Deserialize, kithara_config::Config)]
#[serde(default, deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct ChainConfig {
    #[config(value)]
    pub(super) arm_containment: f64,
    /// A decision row needs this Jaccard of its sides, or `arm_containment`.
    #[config(value)]
    pub(super) arm_jaccard: f64,
    /// Impls of one trait method form a row only when their regions reach
    /// this Jaccard, or their best function pair `dyn_pair`.
    #[config(value)]
    pub(super) dyn_jaccard: f64,
    #[config(value)]
    pub(super) dyn_pair: f64,
    /// Jaccard that pairs two roots, or the regions two roots own.
    #[config(value)]
    pub(super) similarity: f64,
    /// Calls walked back to the common caller of two roots and forward from
    /// it to each root, and rounds of passing a cfg from callers to callees.
    /// A side region is not bounded: it holds every function only it calls.
    #[config(value)]
    pub(super) depth: usize,
    /// A trait-object call with more impls is a hub and links nowhere.
    #[config(value)]
    pub(super) max_dyn_targets: usize,
    /// A variant handled in more functions is a hub and links nowhere.
    #[config(value)]
    pub(super) max_variant_handlers: usize,
    /// Lines on each side.
    #[config(value)]
    pub(super) min_side_lines: usize,
    /// Tokens a function needs before it is compared at all.
    #[config(value)]
    pub(super) min_tokens: usize,
    /// Functions, and arms with code of their own, on both sides together.
    #[config(value)]
    pub(super) min_units: usize,
    /// Tokens per shingle.
    #[config(value)]
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
