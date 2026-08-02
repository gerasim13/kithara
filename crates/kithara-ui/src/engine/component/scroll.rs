use kithara_platform::time::Instant;
use num_traits::ToPrimitive;

use super::retained::Component;
use crate::{
    engine::model::{EngineEvent, Kind},
    interact::{CursorShape, Hit, Input, Outcome, Scroll, recognizers::click},
};

const LINE_STEP_PX: f32 = 60.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct ScrollState {
    offset: f32,
    row_count: usize,
    row_height: f32,
    row_right_inset: f32,
    viewport_height: f32,
}

impl ScrollState {
    pub(crate) fn new(row_count: usize, row_height: f32, row_right_inset: f32) -> Self {
        Self {
            offset: 0.0,
            row_count,
            row_height,
            row_right_inset,
            viewport_height: 0.0,
        }
    }

    pub(crate) fn reconcile(&mut self, row_count: usize, row_height: f32, row_right_inset: f32) {
        self.row_count = row_count;
        self.row_height = row_height;
        self.row_right_inset = row_right_inset;
        self.clamp_offset();
    }

    pub(crate) const fn offset(&self) -> f32 {
        self.offset
    }

    pub(crate) fn sync_offset(&mut self, offset: f32) {
        self.offset = offset.clamp(0.0, self.max_offset());
    }

    pub(crate) fn set_viewport(&mut self, height: f32) {
        self.viewport_height = height.max(0.0);
        self.clamp_offset();
    }

    pub(crate) fn handle(&mut self, input: Input, hit: &Hit) -> Outcome<usize> {
        if let Input::Wheel(scroll) = input {
            return self.wheel(scroll, hit);
        }
        if click::on_input(input, hit) == Outcome::IGNORED {
            return Outcome::IGNORED;
        }
        self.row_at(hit).map_or(Outcome::IGNORED, Outcome::set)
    }

    fn wheel(&mut self, scroll: Scroll, hit: &Hit) -> Outcome<usize> {
        if !hit.over() {
            return Outcome::IGNORED;
        }
        self.set_viewport(hit.area().h);
        let delta = match scroll {
            Scroll::Lines(y) => y * LINE_STEP_PX,
            Scroll::Pixels(y) => y,
        };
        let next = (self.offset - delta).clamp(0.0, self.max_offset());
        if next == self.offset {
            return Outcome::IGNORED;
        }
        self.offset = next;
        Outcome::captured()
    }

    fn row_at(&self, hit: &Hit) -> Option<usize> {
        let point = hit.inside()?;
        if self.row_height <= 0.0 {
            return None;
        }
        let row_right = hit.area().x + (hit.area().w - self.row_right_inset).max(0.0);
        if self.max_offset() > 0.0 && point.x >= row_right {
            return None;
        }
        let y = point.y - hit.area().y + self.offset;
        if y < 0.0 {
            return None;
        }
        let index = (y / self.row_height).floor().to_usize()?;
        (index < self.row_count).then_some(index)
    }

    fn clamp_offset(&mut self) {
        self.offset = self.offset.clamp(0.0, self.max_offset());
    }

    fn max_offset(&self) -> f32 {
        let rows = self.row_count.to_f32().unwrap_or(f32::MAX);
        rows.mul_add(self.row_height.max(0.0), -self.viewport_height)
            .max(0.0)
    }
}

pub(in crate::engine) struct ScrollComponent {
    path: String,
    state: ScrollState,
}

impl ScrollComponent {
    pub(super) fn new(
        path: String,
        row_count: usize,
        row_height: f32,
        row_right_inset: f32,
    ) -> Self {
        Self {
            path,
            state: ScrollState::new(row_count, row_height, row_right_inset),
        }
    }

    pub(super) fn reconcile(mut self, next: Self) -> Self {
        self.path = next.path;
        self.state.reconcile(
            next.state.row_count,
            next.state.row_height,
            next.state.row_right_inset,
        );
        self
    }

    pub(super) const fn offset(&self) -> f32 {
        self.state.offset()
    }

    pub(super) fn set_viewport(&mut self, height: f32) {
        self.state.set_viewport(height);
    }
}

impl Component for ScrollComponent {
    fn path(&self) -> &str {
        &self.path
    }

    fn kind(&self) -> Kind {
        Kind::Scroll
    }

    fn handle(
        &mut self,
        input: Input,
        hit: &Hit,
        _now: Instant,
    ) -> (Outcome<EngineEvent>, Option<&'static str>) {
        (self.state.handle(input, hit).map(EngineEvent::Index), None)
    }

    fn cursor(&self, hit: &Hit) -> CursorShape {
        if self.state.row_at(hit).is_some() {
            CursorShape::Pointer
        } else {
            CursorShape::None
        }
    }

    fn captures_pointer(&self) -> bool {
        false
    }
}
