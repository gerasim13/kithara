use crate::interact::{Hit, Outcome};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Kind {
    Activation,
    Crossfader,
    Knob,
    StereoMeter,
    VerticalVu,
    Wave,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Identity {
    pub(super) path: String,
    pub(super) kind: Kind,
}

pub(crate) enum Descriptor {
    Activation {
        path: String,
    },
    Crossfader {
        path: String,
    },
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
    Wave {
        path: String,
        beats_shown: bool,
        scale: f32,
        progress: f32,
    },
}

impl Descriptor {
    pub(crate) fn activation(path: String) -> Self {
        Self::Activation { path }
    }

    pub(crate) fn crossfader(path: String) -> Self {
        Self::Crossfader { path }
    }

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

    pub(crate) fn wave(path: String, beats_shown: bool, scale: f32, progress: f32) -> Self {
        Self::Wave {
            path,
            beats_shown,
            scale,
            progress: progress.clamp(0.0, 1.0),
        }
    }

    pub(super) fn path(&self) -> &str {
        match self {
            Self::Activation { path }
            | Self::Crossfader { path }
            | Self::Knob { path, .. }
            | Self::StereoMeter { path }
            | Self::VerticalVu { path }
            | Self::Wave { path, .. } => path,
        }
    }

    pub(super) const fn kind(&self) -> Kind {
        match self {
            Self::Activation { .. } => Kind::Activation,
            Self::Crossfader { .. } => Kind::Crossfader,
            Self::Knob { .. } => Kind::Knob,
            Self::StereoMeter { .. } => Kind::StereoMeter,
            Self::VerticalVu { .. } => Kind::VerticalVu,
            Self::Wave { .. } => Kind::Wave,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum EngineEvent {
    Scalar(f32),
    Activate,
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
    pub(crate) outcome: Outcome<EngineEvent>,
}

impl Emission {
    pub(crate) const fn is_captured(&self) -> bool {
        self.outcome.is_captured()
    }
}
