use crate::{
    hosts::controls::grip::{Drag, IndexPress, Span},
    interact::recognizers::{Edge, WheelStep},
    render::ScalarRange,
};

impl Span {
    /// The same drag measured against the interval the control now draws.
    pub(crate) const fn at(self, value: ScalarRange) -> Self {
        Self { value, ..self }
    }

    /// The interval that results from moving one of its ends.
    ///
    /// The other end is left exactly where it was, and the two are not ordered:
    /// snapping and the minimum gap between them belong to the host, and a
    /// control that closed the gap itself would fight the answer coming back.
    pub(crate) const fn moved(self, edge: Edge, value: f32) -> ScalarRange {
        match edge {
            Edge::Min => ScalarRange {
                min: value,
                max: self.value.max,
            },
            Edge::Max => ScalarRange {
                min: self.value.min,
                max: value,
            },
        }
    }
}

impl Drag {
    /// The same drag counting from the value the control now draws. Only a
    /// host that keeps its widgets needs this; the other builds a fresh drag
    /// with every frame.
    pub(crate) fn at(self, value: f32) -> Self {
        Self {
            track: self.track.at(value),
            wheel: self.wheel.map(|wheel| WheelStep { value, ..wheel }),
            ..self
        }
    }
}

impl IndexPress {
    pub(crate) fn hover(&mut self, hovered: bool) -> bool {
        if hovered || self.hovered.is_none() {
            return false;
        }
        self.hovered = None;
        true
    }
}
