use crate::interact::{
    CursorShape, Hover,
    recognizers::{Scalar, Track, WheelStep},
};

/// What the pointer means to a control.
#[derive(Clone, Copy)]
pub(crate) enum Grip {
    /// Nothing the control itself recognises: either it is not interactive, or
    /// the engine plan drives it.
    None,
    /// A press that activates it.
    Press,
    /// A drag along one axis that sets a scalar.
    Drag(Drag),
}

/// A scalar drag, described rather than built.
///
/// A host that rebuilds its tree every frame could hold the recognizer itself,
/// because the value it counts from is fresh each time. A host that keeps its
/// widgets cannot: it is told the new value instead, and has to re-make the
/// recognizer from it — which it can only do from the description.
#[derive(Clone, Copy, bon::Builder)]
pub(crate) struct Drag {
    cursor: CursorShape,
    track: Track,
    reset: Option<f32>,
    wheel: Option<WheelStep>,
}

impl Drag {
    pub(crate) fn recognizer(self) -> Scalar {
        Scalar::builder()
            .track(self.track)
            .hover(Hover::new(self.cursor))
            .maybe_reset(self.reset)
            .maybe_wheel(self.wheel)
            .build()
    }

    /// The same drag counting from the value the control now draws. Only a
    /// host that keeps its widgets needs this; the other builds a fresh drag
    /// with every frame.
    #[cfg(feature = "masonry-host")]
    pub(crate) fn at(self, value: f32) -> Self {
        Self {
            track: self.track.at(value),
            wheel: self.wheel.map(|wheel| WheelStep { value, ..wheel }),
            ..self
        }
    }
}
