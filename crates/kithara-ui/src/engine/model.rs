use std::ops::Range;

use crate::interact::{
    Hit, Hover, Outcome,
    recognizers::{Scalar, Track, WheelStep},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Kind {
    Activation,
    Crossing,
    Segmented,
    Scroll,
    Fader,
    Crossfader,
    Knob,
    StereoMeter,
    VerticalVu,
    Wave,
    HeroWave,
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
    Crossing {
        path: String,
    },
    Segmented {
        path: String,
        item_count: usize,
    },
    Scroll {
        path: String,
        row_count: usize,
        row_height: f32,
        row_right_inset: f32,
    },
    Fader {
        path: String,
        scalar: Scalar,
        drag_step: Option<f64>,
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
    },
    HeroWave {
        path: String,
        scale: f32,
        progress: f32,
        visible: Range<f32>,
        wheel_positive: f32,
        wheel_non_positive: f32,
    },
}

impl Descriptor {
    pub(crate) fn activation(path: String) -> Self {
        Self::Activation { path }
    }

    pub(crate) fn crossing(path: String) -> Self {
        Self::Crossing { path }
    }

    pub(crate) fn segmented(path: String, item_count: usize) -> Self {
        Self::Segmented { path, item_count }
    }

    pub(crate) fn scroll(
        path: String,
        row_count: usize,
        row_height: f32,
        row_right_inset: f32,
    ) -> Self {
        Self::Scroll {
            path,
            row_count,
            row_height,
            row_right_inset,
        }
    }

    pub(crate) fn fader(
        path: String,
        hover: Hover,
        drag_step: Option<f64>,
        wheel: Option<WheelStep>,
    ) -> Self {
        Self::Fader {
            path,
            scalar: Scalar::builder()
                .track(Track::AbsoluteHorizontal)
                .hover(hover)
                .maybe_wheel(wheel)
                .build(),
            drag_step,
        }
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

    pub(crate) fn wave(path: String) -> Self {
        Self::Wave { path }
    }

    pub(crate) fn hero_wave(
        path: String,
        scale: f32,
        progress: f32,
        visible: Range<f32>,
        wheel_positive: f32,
        wheel_non_positive: f32,
    ) -> Self {
        Self::HeroWave {
            path,
            scale,
            progress: progress.clamp(0.0, 1.0),
            visible,
            wheel_positive,
            wheel_non_positive,
        }
    }

    pub(super) fn path(&self) -> &str {
        match self {
            Self::Activation { path }
            | Self::Crossing { path }
            | Self::Segmented { path, .. }
            | Self::Scroll { path, .. }
            | Self::Fader { path, .. }
            | Self::Crossfader { path }
            | Self::Knob { path, .. }
            | Self::StereoMeter { path }
            | Self::VerticalVu { path }
            | Self::Wave { path, .. }
            | Self::HeroWave { path, .. } => path,
        }
    }

    pub(super) const fn kind(&self) -> Kind {
        match self {
            Self::Activation { .. } => Kind::Activation,
            Self::Crossing { .. } => Kind::Crossing,
            Self::Segmented { .. } => Kind::Segmented,
            Self::Scroll { .. } => Kind::Scroll,
            Self::Fader { .. } => Kind::Fader,
            Self::Crossfader { .. } => Kind::Crossfader,
            Self::Knob { .. } => Kind::Knob,
            Self::StereoMeter { .. } => Kind::StereoMeter,
            Self::VerticalVu { .. } => Kind::VerticalVu,
            Self::Wave { .. } => Kind::Wave,
            Self::HeroWave { .. } => Kind::HeroWave,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum EngineEvent {
    Scalar(f64),
    Activate,
    Crossing(bool),
    Index(usize),
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
    pub(crate) child: Option<&'static str>,
    pub(crate) outcome: Outcome<EngineEvent>,
}
