mod controls;
#[cfg(not(target_arch = "wasm32"))]
mod processor;
#[cfg(not(target_arch = "wasm32"))]
mod processor_effect;
#[cfg(not(target_arch = "wasm32"))]
mod processor_render;
#[cfg(not(target_arch = "wasm32"))]
mod processor_target;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod region_tests;
pub use controls::StretchControls;
#[cfg(not(target_arch = "wasm32"))]
pub use kithara_stretch::{ElasticEngine, ElasticError, StretchKind};
#[cfg(not(target_arch = "wasm32"))]
pub use processor::TimeStretchProcessor;

pub use crate::region::{RegionPlan, RegionPlanError};
