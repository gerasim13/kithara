use super::{ir::DrawCmd, list::DrawList, paint::Paint};

/// What a backend is able to draw.
///
/// Every backend states this once, as a constant. A backend that cannot do
/// something does not approximate it and does not skip it: the list that asks
/// is refused whole, before anything reaches the screen.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Caps {
    /// Scopes drawing to a rectangle.
    pub clip: bool,
    /// Ramps a fill along a line.
    pub linear_gradient: bool,
    /// Fills an outline that no named shape covers.
    pub outline: bool,
}

impl Caps {
    /// A backend that draws everything the list can express.
    pub const EVERYTHING: Self = Self {
        clip: true,
        linear_gradient: true,
        outline: true,
    };

    /// Refuses a list this backend cannot draw whole.
    ///
    /// # Errors
    /// Returns the first capability the list needs and this backend lacks.
    pub const fn accepts(self, needs: Needs) -> Result<(), Unsupported> {
        if needs.clip && !self.clip {
            return Err(Unsupported::Clip);
        }
        if needs.outline && !self.outline {
            return Err(Unsupported::Outline);
        }
        if needs.linear_gradient && !self.linear_gradient {
            return Err(Unsupported::LinearGradient);
        }
        Ok(())
    }
}

/// What a list needs its backend to be able to do.
///
/// Read off the list itself, so the question is answered by what is actually
/// drawn rather than by what a document might in principle contain.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Needs {
    pub(super) clip: bool,
    pub(super) linear_gradient: bool,
    pub(super) outline: bool,
}

impl Needs {
    fn read(&mut self, list: &DrawList) {
        for command in list.commands() {
            match command {
                DrawCmd::Clip { list, .. } => {
                    self.clip = true;
                    self.read(list);
                }
                DrawCmd::Fill { geom, paint } => {
                    self.outline |= geom.is_outline();
                    self.linear_gradient |= matches!(paint, Paint::Linear { .. });
                }
                DrawCmd::Stroke { geom, .. } => self.outline |= geom.is_outline(),
                DrawCmd::Text { .. } => {}
            }
        }
    }
}

impl From<&DrawList> for Needs {
    fn from(list: &DrawList) -> Self {
        let mut needs = Self::default();
        needs.read(list);
        needs
    }
}

/// What a list asked for that its backend cannot draw.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum Unsupported {
    #[error("this backend cannot scope drawing to a clip")]
    Clip,
    #[error("this backend cannot ramp a fill along a line")]
    LinearGradient,
    #[error("this backend cannot fill an outline")]
    Outline,
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::{Caps, Needs, Unsupported};
    use crate::draw::{DrawListBuilder, FillRule, Paint, Path, Pt, Rect, Rgba, Stop, Stops, Verb};

    fn ink() -> Rgba {
        Rgba {
            a: 1.0,
            b: 1.0,
            g: 1.0,
            r: 1.0,
        }
    }

    fn unit_box() -> Rect {
        Rect {
            h: 10.0,
            w: 10.0,
            x: 0.0,
            y: 0.0,
        }
    }

    fn outline() -> Path {
        Path::new(
            FillRule::NonZero,
            vec![Verb::MoveTo(Pt { x: 0.0, y: 0.0 }), Verb::Close],
        )
    }

    fn ramp() -> Paint {
        let stops = Stops::new(&[
            Stop {
                color: ink(),
                offset: 0.0,
            },
            Stop {
                color: ink(),
                offset: 1.0,
            },
        ])
        .unwrap_or_else(|error| panic!("a two-stop ramp is valid: {error}"));
        Paint::Linear {
            from: Pt { x: 0.0, y: 0.0 },
            stops,
            to: Pt { x: 1.0, y: 0.0 },
        }
    }

    /// A list of plain filled rectangles asks nothing of anyone.
    #[kithara::test]
    fn a_plain_list_needs_nothing_in_particular() {
        let mut list = DrawListBuilder::default();
        list.fill_rect(unit_box(), ink());

        assert_eq!(Needs::from(&list.finish()), Needs::default());
    }

    /// What a list needs is read from what it draws, however deeply nested.
    #[kithara::test]
    fn a_need_inside_a_clip_still_counts() {
        let mut inner = DrawListBuilder::default();
        inner.fill_path(outline(), ink());
        let mut list = DrawListBuilder::default();
        list.clip(unit_box(), inner.finish());
        let needs = Needs::from(&list.finish());

        assert_eq!(Caps::EVERYTHING.accepts(needs), Ok(()));
        assert_eq!(
            Caps {
                clip: false,
                ..Caps::EVERYTHING
            }
            .accepts(needs),
            Err(Unsupported::Clip)
        );
        assert_eq!(
            Caps {
                outline: false,
                ..Caps::EVERYTHING
            }
            .accepts(needs),
            Err(Unsupported::Outline)
        );
    }

    /// A backend without gradients refuses the document rather than flattening
    /// the ramp, which would be a different picture nobody asked for.
    #[kithara::test]
    fn a_ramp_is_refused_where_it_cannot_be_drawn() {
        let mut list = DrawListBuilder::default();
        list.fill_rect(unit_box(), ramp());
        let needs = Needs::from(&list.finish());

        assert_eq!(Caps::EVERYTHING.accepts(needs), Ok(()));
        assert_eq!(
            Caps {
                linear_gradient: false,
                ..Caps::EVERYTHING
            }
            .accepts(needs),
            Err(Unsupported::LinearGradient)
        );
    }
}
