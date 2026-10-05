use kithara_platform::time::Instant;

use super::retained::Component;
use crate::{
    engine::model::{EngineEvent, Kind, Press},
    interact::{CursorShape, Hit, Hover, Input, Outcome, recognizers::click},
};

pub(in crate::engine) struct ActivationComponent {
    hover: Hover,
    path: String,
    press: Press,
}

impl ActivationComponent {
    pub(super) fn new(path: String, press: Press) -> Self {
        Self {
            path,
            press,
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
        index: Option<usize>,
        _now: Instant,
    ) -> (Outcome<EngineEvent>, Option<&'static str>) {
        if !click::on_input(input, hit).is_captured() {
            return (Outcome::IGNORED, None);
        }
        let outcome = match &self.press {
            Press::Activate => Outcome::set(EngineEvent::Activate),
            Press::Texts(texts) => index
                .and_then(|index| texts.get(index))
                .and_then(Option::as_ref)
                .map_or(Outcome::IGNORED, |text| {
                    Outcome::set(EngineEvent::Text(text.clone()))
                }),
        };
        (outcome, None)
    }

    fn kind(&self) -> Kind {
        Kind::Activation
    }

    fn path(&self) -> &str {
        &self.path
    }
}
