use kithara_platform::time::Instant;

use super::component::Component;
use crate::interact::{
    CursorShape, Hit, Hover, Input, Outcome,
    recognizers::{Scalar, ScalarState, Track, WheelStep},
};

pub(super) struct KnobComponent {
    path: String,
    drag: Scalar,
    state: ScalarState,
}

impl KnobComponent {
    pub(super) fn new(path: String, current: f32, drag_range: f32, wheel_step: f32) -> Self {
        Self {
            path,
            drag: knob_drag(current, drag_range, wheel_step),
            state: ScalarState::default(),
        }
    }

    pub(super) fn refresh(&mut self, path: String, current: f32, drag_range: f32, wheel_step: f32) {
        self.path = path;
        self.drag = knob_drag(current, drag_range, wheel_step);
    }
}

impl Component for KnobComponent {
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

fn knob_drag(current: f32, drag_range: f32, wheel_step: f32) -> Scalar {
    Scalar::builder()
        .track(Track::RelativeVertical {
            range: drag_range,
            value: current,
        })
        .hover(Hover::new(CursorShape::ResizeV))
        .reset(0.5)
        .wheel(WheelStep {
            value: current,
            step: wheel_step,
        })
        .build()
}
