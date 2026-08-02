use crate::draw::{Pt, Rect};

#[derive(Clone, Copy)]
pub(crate) enum Input<'a> {
    KeyPressed {
        key: Key<'a>,
        modifiers: Modifiers,
        text: Option<&'a str>,
    },
    KeyReleased {
        key: Key<'a>,
        modifiers: Modifiers,
    },
    InputMethod(InputMethod<'a>),
    ModifiersChanged(Modifiers),
    PointerDown,
    /// `at` is where the host says the pointer went. It answers a different
    /// question from [`Hit::at`] and the two are not interchangeable: this one
    /// is always reported, even while a widget is told it has no cursor, but it
    /// is only comparable against itself. A recognizer measuring travel reads
    /// it; one normalizing against an area must read the hit, which is the
    /// position expressed in that area's space.
    PointerMoved {
        at: Pt,
    },
    PointerLeft,
    PointerUp,
    Wheel(Scroll),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Key<'a> {
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    Backspace,
    Delete,
    End,
    Enter,
    Escape,
    Home,
    Space,
    Character(&'a str),
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InputMethod<'a> {
    Opened,
    Preedit {
        content: &'a str,
        selection: Option<(usize, usize)>,
    },
    Commit(&'a str),
    Closed,
}

#[derive(Clone, Copy, Default, Eq, PartialEq)]
pub(crate) struct Modifiers {
    alt: bool,
    control: bool,
    logo: bool,
    shift: bool,
}

impl Modifiers {
    pub(crate) const fn new(alt: bool, control: bool, logo: bool, shift: bool) -> Self {
        Self {
            alt,
            control,
            logo,
            shift,
        }
    }

    pub(crate) const fn shift(self) -> bool {
        self.shift
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ScrollAxis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy)]
pub(crate) enum Scroll {
    Lines { x: f32, y: f32 },
    Pixels { x: f32, y: f32 },
}

impl Scroll {
    pub(crate) const fn lines(y: f32) -> Self {
        Self::Lines { x: 0.0, y }
    }

    #[cfg(test)]
    pub(crate) const fn pixels(y: f32) -> Self {
        Self::Pixels { x: 0.0, y }
    }

    #[cfg(test)]
    pub(crate) const fn x(self) -> f32 {
        self.delta(ScrollAxis::Horizontal)
    }

    pub(crate) const fn y(self) -> f32 {
        self.delta(ScrollAxis::Vertical)
    }

    pub(crate) const fn delta(self, axis: ScrollAxis) -> f32 {
        let (x, y) = match self {
            Self::Lines { x, y } | Self::Pixels { x, y } => (x, y),
        };
        match axis {
            ScrollAxis::Horizontal => x,
            ScrollAxis::Vertical => y,
        }
    }

    pub(crate) const fn is_pixels(self) -> bool {
        matches!(self, Self::Pixels { .. })
    }
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

    pub(crate) fn uniform_horizontal_index(self, count: usize) -> Option<usize> {
        self.area.uniform_horizontal_index(self.inside()?, count)
    }

    /// The box the pointer is tested against, for a recognizer that normalizes
    /// a position against it rather than only asking whether it landed inside.
    pub(crate) const fn area(self) -> Rect {
        self.area
    }
}
