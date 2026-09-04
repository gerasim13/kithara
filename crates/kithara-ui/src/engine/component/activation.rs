use kithara_platform::time::Instant;

use super::retained::Component;
use crate::{
    engine::model::{EngineEvent, Kind},
    interact::{CursorShape, Hit, Hover, Input, Outcome, recognizers::click},
};

pub(in crate::engine) struct ActivationComponent {
    hover: Hover,
    path: String,
}

impl ActivationComponent {
    pub(super) fn new(path: String) -> Self {
        Self {
            path,
            hover: Hover::new(CursorShape::Pointer),
        }
    }
}

impl Component for ActivationComponent {
    fn captures_pointer(&self) -> bool {
        false
    }

    fn cursor(&self, hit: &Hit) -> CursorShape {
        self.hover.cursor(false, hit)
    }

    fn handle(
        &mut self,
        input: Input<'_>,
        hit: &Hit,
        _index: Option<usize>,
        _now: Instant,
    ) -> (Outcome<EngineEvent>, Option<&'static str>) {
        (
            click::on_input(input, hit).map(|()| EngineEvent::Activate),
            None,
        )
    }

    fn kind(&self) -> Kind {
        Kind::Activation
    }

    fn path(&self) -> &str {
        &self.path
    }
}
