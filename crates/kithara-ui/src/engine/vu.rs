use kithara_platform::time::Instant;

use super::component::Component;
use crate::interact::{
    CursorShape, Hit, Hover, Input, Outcome,
    recognizers::{Scalar, ScalarState, Track},
};

pub(super) struct VerticalVuComponent {
    path: String,
    drag: Scalar,
    state: ScalarState,
}

impl VerticalVuComponent {
    pub(super) fn new(path: String) -> Self {
        Self {
            path,
            drag: vertical_vu_drag(),
            state: ScalarState::default(),
        }
    }

    pub(super) fn refresh(&mut self, path: String) {
        self.path = path;
        self.drag = vertical_vu_drag();
    }
}

impl Component for VerticalVuComponent {
    fn path(&self) -> &str {
        &self.path
    }

    fn handle(&mut self, input: Input, hit: &Hit, now: Instant) -> Outcome<f32> {
        self.drag.on_input(&mut self.state, input, hit, now)
    }

    fn cursor(&self, hit: &Hit) -> CursorShape {
        self.drag.cursor(&self.state, hit)
    }

    fn captures_pointer(&self) -> bool {
        self.state.captures_pointer()
    }
}

fn vertical_vu_drag() -> Scalar {
    Scalar::builder()
        .track(Track::AbsoluteVertical)
        .hover(Hover::new(CursorShape::ResizeV))
        .build()
}
