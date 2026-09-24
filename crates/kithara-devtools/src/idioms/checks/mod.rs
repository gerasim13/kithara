//! Registry of idiom checks (constructions that hint at a better Rust pattern).
//!
//! `idioms` is the third static-analysis namespace alongside `arch` (topology)
//! and `style` (intra-file organisation). It flags constructions that compile
//! and pass clippy but are worth reconsidering for performance, readability,
//! or expressivity.

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
mod registry;
pub(crate) mod retry_fallback;
pub(crate) mod thin_wrapper_economy;

pub(crate) use registry::{Check, CheckPolicy, Context, registry};
