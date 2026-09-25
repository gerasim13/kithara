use num_traits::cast::ToPrimitive;

use super::consts;

pub(crate) fn frame_seconds() -> f32 {
    consts::frames::HOP.to_f32().unwrap_or(1.0) / consts::frames::RATE
}

pub(crate) fn seconds(frame: f32) -> f32 {
    frame * consts::frames::HOP.to_f32().unwrap_or(1.0) / consts::frames::RATE
}
