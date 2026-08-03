use crate::{
    draw::{DrawList, Pt, Rect},
    interact::{CursorShape, Hit, Input, Outcome},
};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HostLayer<A> {
    bounds: Rect,
    draw: DrawList,
    hits: Vec<LayerHit<A>>,
}

impl<A> HostLayer<A> {
    pub(crate) const fn new(bounds: Rect, draw: DrawList, hits: Vec<LayerHit<A>>) -> Self {
        Self { bounds, draw, hits }
    }

    pub(crate) const fn bounds(&self) -> Rect {
        self.bounds
    }

    pub(crate) const fn draw(&self) -> &DrawList {
        &self.draw
    }

    pub(crate) fn hits(&self) -> &[LayerHit<A>] {
        &self.hits
    }

    pub(crate) fn handle(&self, input: Input<'_>, pointer: Option<Pt>) -> Outcome<A>
    where
        A: Copy,
    {
        if !matches!(input, Input::PointerDown) {
            return Outcome::IGNORED;
        }
        self.hit(pointer)
            .map_or(Outcome::IGNORED, |hit| Outcome::set(*hit.action()))
    }

    pub(crate) fn cursor_at(&self, pointer: Option<Pt>) -> CursorShape {
        self.hit(pointer)
            .map_or(CursorShape::None, LayerHit::cursor)
    }

    pub(crate) fn action_at(&self, pointer: Option<Pt>) -> Option<&A> {
        self.hit(pointer).map(LayerHit::action)
    }

    fn hit(&self, pointer: Option<Pt>) -> Option<&LayerHit<A>> {
        self.hits()
            .iter()
            .rev()
            .find(|region| Hit::new(pointer, region.area).over())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LayerHit<A> {
    area: Rect,
    cursor: CursorShape,
    action: A,
}

impl<A> LayerHit<A> {
    pub(crate) const fn new(area: Rect, cursor: CursorShape, action: A) -> Self {
        Self {
            area,
            cursor,
            action,
        }
    }

    pub(crate) const fn area(&self) -> Rect {
        self.area
    }

    pub(crate) const fn cursor(&self) -> CursorShape {
        self.cursor
    }

    pub(crate) const fn action(&self) -> &A {
        &self.action
    }
}

pub(crate) fn handle<A: Copy>(
    layers: &[HostLayer<A>],
    input: Input<'_>,
    pointer: Option<Pt>,
) -> Outcome<A> {
    layers
        .iter()
        .rev()
        .map(|layer| layer.handle(input, pointer))
        .find(Outcome::is_captured)
        .unwrap_or(Outcome::IGNORED)
}

pub(crate) fn cursor<A>(layers: &[HostLayer<A>], pointer: Option<Pt>) -> CursorShape {
    layers
        .iter()
        .rev()
        .map(|layer| layer.cursor_at(pointer))
        .find(|shape| *shape != CursorShape::None)
        .unwrap_or(CursorShape::None)
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        draw::{DrawList, Pt, Rect},
        interact::{CursorShape, Input, Outcome},
    };

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { h, w, x, y }
    }

    #[kithara::test]
    fn the_last_layer_is_topmost_for_input_and_cursor() {
        let lower = HostLayer::new(
            rect(0.0, 0.0, 100.0, 60.0),
            DrawList::default(),
            vec![LayerHit::new(
                rect(0.0, 0.0, 8.0, 60.0),
                CursorShape::ResizeH,
                1_u8,
            )],
        );
        let upper = HostLayer::new(
            rect(0.0, 0.0, 100.0, 60.0),
            DrawList::default(),
            vec![LayerHit::new(
                rect(0.0, 0.0, 8.0, 60.0),
                CursorShape::Pointer,
                2_u8,
            )],
        );
        let layers = [lower, upper];
        let pointer = Some(Pt { x: 3.0, y: 30.0 });

        assert_eq!(
            handle(&layers, Input::PointerDown, pointer),
            Outcome::set(2)
        );
        assert_eq!(cursor(&layers, pointer), CursorShape::Pointer);
    }

    #[kithara::test]
    fn a_hit_is_half_open_and_a_non_press_is_ignored() {
        let layer = HostLayer::new(
            rect(0.0, 0.0, 100.0, 60.0),
            DrawList::default(),
            vec![LayerHit::new(
                rect(0.0, 0.0, 4.0, 60.0),
                CursorShape::ResizeH,
                7_u8,
            )],
        );
        let layers = [layer];

        assert_eq!(
            handle(&layers, Input::PointerDown, Some(Pt { x: 3.0, y: 30.0 })),
            Outcome::set(7)
        );
        assert_eq!(
            handle(&layers, Input::PointerDown, Some(Pt { x: 4.0, y: 30.0 })),
            Outcome::IGNORED
        );
        assert_eq!(
            handle(
                &layers,
                Input::PointerMoved {
                    at: Pt { x: 3.0, y: 30.0 }
                },
                Some(Pt { x: 3.0, y: 30.0 })
            ),
            Outcome::IGNORED
        );
    }
}
