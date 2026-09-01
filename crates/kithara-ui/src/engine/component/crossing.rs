use kithara_platform::time::Instant;

use super::retained::Component;
use crate::{
    engine::model::{EngineEvent, Kind},
    interact::{CursorShape, Hit, Input, Outcome, recognizers::Crossing},
};

pub(in crate::engine) struct CrossingComponent {
    crossing: Crossing,
    path: String,
}

impl CrossingComponent {
    pub(super) fn new(path: String) -> Self {
        Self {
            path,
            crossing: Crossing::default(),
        }
    }
}

impl Component for CrossingComponent {
    fn captures_pointer(&self) -> bool {
        false
    }

    fn cursor(&self, _hit: &Hit) -> CursorShape {
        CursorShape::None
    }

    fn handle(
        &mut self,
        input: Input<'_>,
        hit: &Hit,
        _index: Option<usize>,
        _now: Instant,
    ) -> (Outcome<EngineEvent>, Option<&'static str>) {
        (
            self.crossing
                .on_input(input, hit)
                .map(EngineEvent::Crossing),
            None,
        )
    }

    fn kind(&self) -> Kind {
        Kind::Crossing
    }

    fn path(&self) -> &str {
        &self.path
    }
}
