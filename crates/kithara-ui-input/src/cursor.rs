use super::Hit;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "iced", derive(kithara_derive::Mirror))]
#[cfg_attr(feature = "iced", mirror(into = iced::mouse::Interaction))]
pub enum CursorShape {
    None,
    Grab,
    Grabbing,
    Pointer,
    #[cfg_attr(feature = "iced", mirror(rename = ResizingDiagonallyDown))]
    ResizeDiagonalDown,
    #[cfg_attr(feature = "iced", mirror(rename = ResizingDiagonallyUp))]
    ResizeDiagonalUp,
    #[cfg_attr(feature = "iced", mirror(rename = ResizingHorizontally))]
    ResizeH,
    #[cfg_attr(feature = "iced", mirror(rename = ResizingVertically))]
    ResizeV,
    Text,
}

#[derive(Clone, Copy)]
pub struct Hover {
    shape: CursorShape,
}

impl Hover {
    #[must_use]
    pub const fn new(shape: CursorShape) -> Self {
        Self { shape }
    }

    #[must_use]
    pub fn cursor(self, active: bool, hit: &Hit) -> CursorShape {
        if active || hit.over() {
            self.shape
        } else {
            CursorShape::None
        }
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;
    use kithara_ui_draw::{Pt, Rect};

    use super::*;

    fn hit(at: Option<Pt>) -> Hit {
        Hit::new(
            at,
            Rect {
                h: 34.0,
                w: 34.0,
                x: 0.0,
                y: 0.0,
            },
        )
    }

    #[kithara::test]
    fn the_cursor_shape_follows_hover_or_an_active_gesture() {
        let hover = Hover::new(CursorShape::ResizeV);

        assert_eq!(
            hover.cursor(false, &hit(Some(Pt { x: 17.0, y: 17.0 }))),
            CursorShape::ResizeV
        );
        assert_eq!(
            hover.cursor(false, &hit(Some(Pt { x: 200.0, y: 200.0 }))),
            CursorShape::None
        );
        assert_eq!(
            hover.cursor(true, &hit(Some(Pt { x: 200.0, y: 200.0 }))),
            CursorShape::ResizeV,
            "an active gesture keeps its shape once the pointer leaves"
        );
    }
}
