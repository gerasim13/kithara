//! Pure synchronization mathematics over track grids and session tempo steps.
#![forbid(unsafe_code)]

mod error;
mod math;
mod trajectory;

pub use error::TrajectoryError;
pub use kithara_signal::Bound;
pub use kithara_warp::{Tempo, TempoError};
pub use math::{
    CorrectionPlan, CorrectionStep, PhaseError, SourceGrid, checked_correction,
    corrected_speed_curve, covers, entry, jump_target, phase_error, speed, speed_curve,
};
pub use trajectory::{TempoStep, TempoTrajectory};
