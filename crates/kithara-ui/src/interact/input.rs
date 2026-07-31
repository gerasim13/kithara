use crate::draw::{Pt, Rect};

#[derive(Clone, Copy)]
pub(crate) enum Input {
    PointerDown,
    PointerMoved,
    PointerUp,
    Wheel(Scroll),
}

#[derive(Clone, Copy)]
pub(crate) enum Scroll {
    Lines(f32),
    Pixels(f32),
}

#[derive(Clone, Copy)]
pub(crate) struct Hit {
    at: Option<Pt>,
    area: Rect,
}

impl Hit {
    pub(crate) const fn new(at: Option<Pt>, area: Rect) -> Self {
        Self { at, area }
    }

    /// The pointer wherever it is, in or out of the area.
    ///
    /// A gesture already under way tracks it past the edge, which is why this
    /// is separate from [`Self::inside`].
    pub(crate) const fn at(self) -> Option<Pt> {
        self.at
    }

    /// The pointer, only while it is within the area.
    ///
    /// A recognizer that starts a gesture needs the position and needs it to
    /// be inside, so one call answers both and leaves no unreachable arm.
    pub(crate) fn inside(self) -> Option<Pt> {
        self.at.filter(|point| self.area.contains(*point))
    }

    pub(crate) fn over(self) -> bool {
        self.inside().is_some()
    }

    /// The box the pointer is tested against, for a recognizer that normalizes
    /// a position against it rather than only asking whether it landed inside.
    pub(crate) const fn area(self) -> Rect {
        self.area
    }
}
