use crate::interact::{Hit, Outcome};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Kind {
    Knob,
    StereoMeter,
    VerticalVu,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Identity {
    pub(super) path: String,
    pub(super) kind: Kind,
}

pub(crate) enum Descriptor {
    Knob {
        path: String,
        current: f32,
        drag_range: f32,
        wheel_step: f32,
    },
    StereoMeter {
        path: String,
    },
    VerticalVu {
        path: String,
    },
}

impl Descriptor {
    pub(crate) fn knob(path: String, current: f32, drag_range: f32, wheel_step: f32) -> Self {
        Self::Knob {
            path,
            current: current.clamp(0.0, 1.0),
            drag_range,
            wheel_step,
        }
    }

    pub(crate) fn vertical_vu(path: String) -> Self {
        Self::VerticalVu { path }
    }

    pub(crate) fn stereo_meter(path: String) -> Self {
        Self::StereoMeter { path }
    }

    pub(super) fn path(&self) -> &str {
        match self {
            Self::Knob { path, .. } | Self::StereoMeter { path } | Self::VerticalVu { path } => {
                path
            }
        }
    }

    pub(super) const fn kind(&self) -> Kind {
        match self {
            Self::Knob { .. } => Kind::Knob,
            Self::StereoMeter { .. } => Kind::StereoMeter,
            Self::VerticalVu { .. } => Kind::VerticalVu,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Target<'a> {
    pub(crate) path: &'a str,
    pub(crate) hit: Hit,
}

impl<'a> Target<'a> {
    pub(crate) const fn new(path: &'a str, hit: Hit) -> Self {
        Self { path, hit }
    }
}

pub(crate) struct Emission {
    pub(crate) path: String,
    pub(crate) outcome: Outcome,
}
