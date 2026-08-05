use super::ir::Pt;

/// One move a vector outline is made of, in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum Verb {
    /// Closes the current subpath back to where it started.
    Close,
    /// A cubic curve through two control points.
    CurveTo {
        first: Pt,
        second: Pt,
        to: Pt,
    },
    LineTo(Pt),
    /// Starts a new subpath.
    MoveTo(Pt),
    /// A quadratic curve through one control point.
    QuadTo {
        control: Pt,
        to: Pt,
    },
}

/// How the inside of an outline that crosses itself is decided.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub enum FillRule {
    /// Inside where the winding number is not zero.
    #[default]
    NonZero,
    /// Inside where a ray crosses the outline an odd number of times, which is
    /// how a shape punches a hole in itself.
    EvenOdd,
}

/// A vector outline: the moves that draw it, and the rule that fills it.
///
/// The named shapes cover what a control's own skin asks for. This covers what
/// a control brings with it — an authored icon, a component's radial cell —
/// which no fixed set of shapes can express, and which would otherwise have to
/// leave the draw list and reach a toolkit directly.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Path {
    rule: FillRule,
    verbs: Vec<Verb>,
}

impl Path {
    #[must_use]
    pub fn new(rule: FillRule, verbs: Vec<Verb>) -> Self {
        Self { rule, verbs }
    }

    #[must_use]
    pub const fn rule(&self) -> FillRule {
        self.rule
    }

    #[must_use]
    pub fn verbs(&self) -> &[Verb] {
        &self.verbs
    }
}
