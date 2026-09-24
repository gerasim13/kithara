use std::path::Path;

use anyhow::Result;
use cargo_metadata::Metadata;

use super::{
    super::config::IdiomsConfig, accumulator_loops, arc_mutex_collection, await_under_guard,
    box_concrete_type, branch_chains, const_group_enum_shape, derivable_built_default,
    derivable_clone, derivable_control, derivable_control_painter, derivable_debug,
    derivable_default, derivable_delegation, derivable_deref, derivable_display,
    derivable_enum_str, derivable_error, derivable_event, derivable_from, derivable_getter,
    derivable_into_probe_arg, derivable_mirror, derivable_node_control, derivable_patch,
    derivable_phase, derivable_ranged, derivable_retained, derivable_serialize,
    derivable_skin_walk, derivable_variants, derivable_view_control, fat_loop_body,
    function_branch_density, guard_cascade, loop_allocation, loop_flag_accumulator,
    manual_question_mark, multi_accumulator_loop, nested_if_let_pyramid, no_passthrough_builder,
    parallel_loops, pointwise_loop, retry_fallback, thin_wrapper_economy,
};
use crate::common::{fix::FixOutcome, scan::Scan, scope::Scope, violation::Violation};

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
