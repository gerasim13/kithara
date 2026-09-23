//! Registry of idiom checks (constructions that hint at a better Rust pattern).
//!
//! `idioms` is the third static-analysis namespace alongside `arch` (topology)
//! and `style` (intra-file organisation). It flags constructions that compile
//! and pass clippy but are worth reconsidering for performance, readability,
//! or expressivity.

use std::path::Path;

use anyhow::Result;
use cargo_metadata::Metadata;

use super::config::IdiomsConfig;
use crate::common::{fix::FixOutcome, scan::Scan, scope::Scope, violation::Violation};

pub(crate) mod accumulator_loops;
pub(crate) mod arc_mutex_collection;
pub(crate) mod await_under_guard;
pub(crate) mod box_concrete_type;
pub(crate) mod branch_chains;
pub(crate) mod const_group_enum_shape;
pub(crate) mod derivable_built_default;
pub(crate) mod derivable_clone;
pub(crate) mod derivable_control;
pub(crate) mod derivable_control_painter;
pub(crate) mod derivable_debug;
pub(crate) mod derivable_default;
pub(crate) mod derivable_delegation;
pub(crate) mod derivable_deref;
pub(crate) mod derivable_display;
pub(crate) mod derivable_enum_str;
pub(crate) mod derivable_error;
pub(crate) mod derivable_event;
pub(crate) mod derivable_from;
pub(crate) mod derivable_getter;
pub(crate) mod derivable_into_probe_arg;
pub(crate) mod derivable_mirror;
pub(crate) mod derivable_node_control;
pub(crate) mod derivable_patch;
pub(crate) mod derivable_phase;
pub(crate) mod derivable_ranged;
pub(crate) mod derivable_retained;
pub(crate) mod derivable_serialize;
pub(crate) mod derivable_skin_walk;
mod derivable_support;
pub(crate) mod derivable_variants;
pub(crate) mod derivable_view_control;
pub(crate) mod fat_loop_body;
pub(crate) mod function_branch_density;
pub(crate) mod guard_cascade;
pub(crate) mod loop_allocation;
pub(crate) mod loop_flag_accumulator;
pub(crate) mod manual_question_mark;
pub(crate) mod multi_accumulator_loop;
pub(crate) mod nested_if_let_pyramid;
pub(crate) mod no_passthrough_builder;
pub(crate) mod parallel_loops;
pub(crate) mod pointwise_loop;
pub(crate) mod retry_fallback;
pub(crate) mod thin_wrapper_economy;

pub(crate) struct Context<'a> {
    pub(crate) config: &'a IdiomsConfig,
    pub(crate) metadata: &'a Metadata,
    pub(crate) workspace_root: &'a Path,
    pub(crate) scan: &'a Scan,
    pub(crate) scope: &'a Scope,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CheckPolicy {
    Default,
    WorkspaceSources,
}

impl CheckPolicy {
    pub(crate) const fn keeps_source_findings(self) -> bool {
        matches!(self, Self::WorkspaceSources)
    }

    pub(crate) fn scope(self, scope: &Scope) -> Scope {
        match self {
            Self::Default => scope.clone(),
            Self::WorkspaceSources => scope.clone().with_workspace_sources(),
        }
    }
}

pub(crate) trait Check: Sync {
    fn fix(&self, _ctx: &Context<'_>) -> Result<FixOutcome> {
        Ok(FixOutcome::default())
    }
    fn id(&self) -> &'static str;
    fn policy(&self) -> CheckPolicy {
        if self.id().starts_with("derivable_") {
            CheckPolicy::WorkspaceSources
        } else {
            CheckPolicy::Default
        }
    }

    fn run(&self, ctx: &Context<'_>) -> Result<Vec<Violation>>;
}

pub(crate) fn registry() -> Vec<Box<dyn Check>> {
    vec![
        Box::new(branch_chains::BranchChains),
        Box::new(guard_cascade::GuardCascade),
        Box::new(derivable_delegation::DerivableDelegation),
        Box::new(derivable_clone::DerivableClone),
        Box::new(derivable_control::DerivableControl),
        Box::new(derivable_control_painter::DerivableControlPainter),
        Box::new(derivable_built_default::DerivableBuiltDefault),
        Box::new(derivable_debug::DerivableDebug),
        Box::new(derivable_default::DerivableDefault),
        Box::new(derivable_from::DerivableFrom),
        Box::new(derivable_ranged::DerivableRanged),
        Box::new(derivable_retained::DerivableRetained),
        Box::new(derivable_serialize::DerivableSerialize),
        Box::new(derivable_skin_walk::DerivableSkinWalk),
        Box::new(derivable_patch::DerivablePatch),
        Box::new(derivable_phase::DerivablePhase),
        Box::new(derivable_view_control::DerivableViewControl),
        Box::new(derivable_deref::DerivableDeref),
        Box::new(derivable_display::DerivableDisplay),
        Box::new(derivable_error::DerivableError),
        Box::new(derivable_event::DerivableEvent),
        Box::new(derivable_getter::DerivableGetter),
        Box::new(derivable_into_probe_arg::DerivableIntoProbeArg),
        Box::new(derivable_mirror::DerivableMirror),
        Box::new(derivable_node_control::DerivableNodeControl),
        Box::new(derivable_enum_str::DerivableEnumStr),
        Box::new(derivable_variants::DerivableVariants),
        Box::new(accumulator_loops::AccumulatorLoops),
        Box::new(multi_accumulator_loop::MultiAccumulatorLoop),
        Box::new(parallel_loops::ParallelLoops),
        Box::new(pointwise_loop::PointwiseLoop),
        Box::new(manual_question_mark::ManualQuestionMark),
        Box::new(loop_allocation::LoopAllocation),
        Box::new(box_concrete_type::BoxConcreteType),
        Box::new(arc_mutex_collection::ArcMutexCollection),
        Box::new(await_under_guard::AwaitUnderGuard),
        Box::new(function_branch_density::FunctionBranchDensity),
        Box::new(retry_fallback::RetryFallback),
        Box::new(fat_loop_body::FatLoopBody),
        Box::new(loop_flag_accumulator::LoopFlagAccumulator),
        Box::new(const_group_enum_shape::ConstGroupEnumShape),
        Box::new(nested_if_let_pyramid::NestedIfLetPyramid),
        Box::new(no_passthrough_builder::NoPassthroughBuilder),
        Box::new(thin_wrapper_economy::ThinWrapperEconomy),
    ]
}
