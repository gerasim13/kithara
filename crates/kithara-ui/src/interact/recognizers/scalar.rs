use kithara_platform::time::Instant;

use super::{
    super::{CursorShape, Hit, Hover, Input, Outcome},
    DoubleClick, wheel,
};

#[derive(bon::Builder)]
pub(crate) struct Scalar {
    value: f32,
    range: f32,
    hover: Hover,
    reset: Option<f32>,
    wheel: Option<WheelStep>,
}

/// Opt-in wheel stepping: the current normalized value plus the per-tick step.
#[derive(Clone, Copy)]
pub(crate) struct WheelStep {
    pub(crate) value: f32,
    pub(crate) step: f32,
}

#[derive(Default)]
pub(crate) struct ScalarState {
    active: bool,
    start_position: f32,
    start_value: f32,
    double_click: DoubleClick,
    wheel_accum: f32,
}

impl Scalar {
    pub(crate) fn on_input(
        &self,
        state: &mut ScalarState,
        input: Input,
        hit: &Hit,
        now: Instant,
    ) -> Outcome {
        match input {
            Input::PointerDown => {
                let Some(position) = hit.inside() else {
                    return Outcome::IGNORED;
                };
                if let Some(value) = self.reset
                    && state.double_click.register(position, now)
                {
                    state.active = false;
                    return Outcome::set(value);
                }
                state.start_position = position.y;
                state.start_value = self.value;
                state.active = true;
                Outcome::captured()
            }
            Input::PointerMoved if state.active => hit.at().map_or(Outcome::IGNORED, |position| {
                Outcome::set(
                    (state.start_value + (state.start_position - position.y) / self.range)
                        .clamp(0.0, 1.0),
                )
            }),
            Input::PointerUp if state.active => {
                state.active = false;
                Outcome::captured()
            }
            Input::Wheel(scroll) if hit.over() => {
                let Some(wheel) = self.wheel else {
                    return Outcome::IGNORED;
                };
                let steps = wheel::steps(&mut state.wheel_accum, scroll);
                if steps == 0.0 {
                    return Outcome::captured();
                }
                let value = wheel.step.mul_add(steps, wheel.value);
                Outcome::set(value.clamp(0.0, 1.0))
            }
            Input::PointerMoved | Input::PointerUp | Input::Wheel(_) => Outcome::IGNORED,
        }
    }

    pub(crate) fn cursor(&self, state: &ScalarState, hit: &Hit) -> CursorShape {
        self.hover.cursor(state.active, hit)
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::{super::super::Scroll, *};
    use crate::draw::{Pt, Rect};

    fn knob() -> Rect {
        Rect {
            h: 34.0,
            w: 34.0,
            x: 0.0,
            y: 0.0,
        }
    }

    fn hit(y: f32) -> Hit {
        Hit::new(Some(Pt { x: 17.0, y }), knob())
    }

    fn drag(value: f32) -> Scalar {
        Scalar::builder()
            .value(value)
            .range(128.0)
            .hover(Hover::new(CursorShape::ResizeV))
            .build()
    }

    fn resetting(reset: f32) -> Scalar {
        Scalar::builder()
            .value(0.8)
            .range(128.0)
            .hover(Hover::new(CursorShape::ResizeV))
            .reset(reset)
            .build()
    }

    fn wheel_drag() -> Scalar {
        Scalar::builder()
            .value(0.5)
            .range(140.0)
            .hover(Hover::new(CursorShape::ResizeV))
            .wheel(WheelStep {
                value: 0.5,
                step: 0.25,
            })
            .build()
    }

    #[kithara::test]
    fn relative_vertical_drag_is_up_positive_and_scaled_by_range() {
        let drag = drag(0.5);
        let now = Instant::now();

        for (from, to, expected) in [(33.0, 1.0, 0.75), (1.0, 33.0, 0.25)] {
            let mut state = ScalarState::default();
            assert_eq!(
                drag.on_input(&mut state, Input::PointerDown, &hit(from), now),
                Outcome::captured()
            );
            assert_eq!(
                drag.on_input(&mut state, Input::PointerMoved, &hit(to), now),
                Outcome::set(expected),
                "{from} -> {to}"
            );
        }
    }

    #[kithara::test]
    fn relative_vertical_press_captures_without_publishing() {
        let drag = drag(0.5);
        let mut state = ScalarState::default();
        let outcome = drag.on_input(&mut state, Input::PointerDown, &hit(17.0), Instant::now());

        assert_eq!(outcome.value(), None, "a relative press seeks nothing");
        assert!(outcome.is_captured());
    }

    #[kithara::test]
    fn a_reset_never_becomes_a_drag() {
        let drag = resetting(0.5);
        let cursor = hit(17.0);
        let mut state = ScalarState::default();
        let now = Instant::now();

        drag.on_input(&mut state, Input::PointerDown, &cursor, now);
        drag.on_input(&mut state, Input::PointerUp, &cursor, now);
        assert_eq!(
            drag.on_input(&mut state, Input::PointerDown, &cursor, now),
            Outcome::set(0.5)
        );
        assert_eq!(
            drag.on_input(&mut state, Input::PointerMoved, &hit(1.0), now),
            Outcome::IGNORED,
            "the press that reset the value must not have armed a drag"
        );
    }

    #[kithara::test]
    fn the_release_after_a_reset_is_not_captured() {
        let drag = resetting(0.5);
        let cursor = hit(17.0);
        let mut state = ScalarState::default();
        let now = Instant::now();

        drag.on_input(&mut state, Input::PointerDown, &cursor, now);
        drag.on_input(&mut state, Input::PointerUp, &cursor, now);
        drag.on_input(&mut state, Input::PointerDown, &cursor, now);

        assert_eq!(
            drag.on_input(&mut state, Input::PointerUp, &cursor, now),
            Outcome::IGNORED,
            "no gesture is active, so the release belongs to whoever is behind"
        );
    }

    #[kithara::test]
    fn relative_drag_double_click_resets_to_configured_value() {
        let drag = resetting(0.5);
        let cursor = hit(17.0);
        let mut state = ScalarState::default();
        let now = Instant::now();

        assert_eq!(
            drag.on_input(&mut state, Input::PointerDown, &cursor, now),
            Outcome::captured()
        );
        assert_eq!(
            drag.on_input(&mut state, Input::PointerUp, &cursor, now),
            Outcome::captured()
        );
        assert_eq!(
            drag.on_input(&mut state, Input::PointerDown, &cursor, now),
            Outcome::set(0.5)
        );
    }

    #[kithara::test]
    fn wheel_steps_the_value_by_direction_and_clamps() {
        let drag = wheel_drag();
        let cursor = hit(17.0);
        let mut state = ScalarState::default();
        let now = Instant::now();

        assert_eq!(
            drag.on_input(&mut state, Input::Wheel(Scroll::Lines(-1.0)), &cursor, now,),
            Outcome::set(0.75)
        );
        assert_eq!(
            drag.on_input(&mut state, Input::Wheel(Scroll::Lines(1.0)), &cursor, now,),
            Outcome::set(0.25)
        );
        assert_eq!(
            drag.on_input(&mut state, Input::Wheel(Scroll::Lines(0.0)), &cursor, now,),
            Outcome::captured(),
            "zero delta must still capture over an opted-in control"
        );
        assert_eq!(
            drag.on_input(
                &mut state,
                Input::Wheel(Scroll::Lines(1.0)),
                &hit(100.0),
                now,
            ),
            Outcome::IGNORED
        );
    }

    #[kithara::test]
    fn trackpad_pixels_accumulate_to_whole_steps() {
        let drag = wheel_drag();
        let cursor = hit(17.0);
        let mut state = ScalarState::default();
        let now = Instant::now();

        assert_eq!(
            drag.on_input(
                &mut state,
                Input::Wheel(Scroll::Pixels(-12.0)),
                &cursor,
                now,
            ),
            Outcome::captured(),
            "sub-threshold pixels capture without publishing"
        );
        assert_eq!(
            drag.on_input(
                &mut state,
                Input::Wheel(Scroll::Pixels(-12.0)),
                &cursor,
                now,
            ),
            Outcome::set(0.75)
        );
        assert_eq!(
            drag.on_input(&mut state, Input::Wheel(Scroll::Pixels(45.0)), &cursor, now,),
            Outcome::set(0.0)
        );
    }
}
