use std::rc::Rc;

use kithara_platform::time::Instant;

use super::custom::HostAction;
use crate::{
    atoms::knob::Knob,
    draw::{DrawList, DrawListBuilder, Rect},
    interact::{
        CursorShape, Hit, Hover, Input, Outcome, PointerOwnership, PointerPhase,
        recognizers::{Scalar, ScalarState, Track, WheelStep},
    },
    render::{ControlAction, Skin, UiEvent, control_event},
    text::TextContext,
};

pub(crate) struct MasonryKnob {
    input: Option<KnobInput>,
    label: Option<String>,
    painter: Knob,
    text: TextContext,
}

struct KnobInput {
    map_event: Rc<dyn Fn(UiEvent) -> HostAction>,
    path: String,
    recognizer: Scalar,
    state: ScalarState,
}

impl MasonryKnob {
    pub(crate) fn new(label: Option<String>, value: f32, skin: &Skin) -> Self {
        Self {
            input: None,
            label,
            painter: Knob::new(value, skin),
            text: TextContext::from(skin.text_resources()),
        }
    }

    pub(crate) fn interactive(
        mut self,
        path: String,
        track: Track,
        wheel: WheelStep,
        map_event: Rc<dyn Fn(UiEvent) -> HostAction>,
    ) -> Self {
        self.input = Some(KnobInput {
            map_event,
            path,
            recognizer: Scalar::builder()
                .track(track)
                .hover(Hover::new(CursorShape::ResizeV))
                .reset(0.5)
                .wheel(wheel)
                .build(),
            state: ScalarState::default(),
        });
        self
    }

    pub(crate) fn draw_list(&mut self, bounds: Rect) -> DrawList {
        let mut list = DrawListBuilder::default();
        self.painter
            .paint(&mut list, &mut self.text, self.label.as_deref(), bounds);
        list.finish()
    }

    pub(crate) fn input(&mut self, input: Input<'_>, hit: &Hit) -> Outcome<HostAction> {
        let Some(state) = &mut self.input else {
            return Outcome::IGNORED;
        };
        let had_pointer = state.state.captures_pointer();
        let outcome = state
            .recognizer
            .on_input(&mut state.state, input, hit, Instant::now());
        if matches!(
            input,
            Input::Pointer(pointer)
                if matches!(pointer.phase, PointerPhase::Cancel | PointerPhase::DoubleClick)
        ) {
            state.state.cancel_pointer();
        }
        let ownership = match (had_pointer, state.state.captures_pointer()) {
            (false, true) => PointerOwnership::Claim,
            (true, false) => PointerOwnership::Release,
            _ => PointerOwnership::Unchanged,
        };
        outcome.with_ownership(ownership).map(|value| {
            (state.map_event)(control_event(
                &state.path,
                ControlAction::SetScalar(f64::from(value)),
            ))
        })
    }

    pub(crate) fn cursor(&self, hit: &Hit) -> CursorShape {
        self.input.as_ref().map_or(CursorShape::None, |input| {
            input.recognizer.cursor(&input.state, hit)
        })
    }

    pub(crate) const fn accepts_input(&self) -> bool {
        self.input.is_some()
    }
}
