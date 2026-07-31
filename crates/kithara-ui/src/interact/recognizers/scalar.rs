use kithara_platform::time::Instant;

use super::{
    super::{CursorShape, Hit, Hover, Input, Outcome},
    DoubleClick, wheel,
};
use crate::draw::{Pt, Rect};

/// How a pointer position becomes a value.
#[derive(Clone, Copy)]
pub(crate) enum Track {
    /// Travel since the press, divided by `range` and added to `value`; up is
    /// positive. The press itself only arms the gesture.
    RelativeVertical { range: f32, value: f32 },
    /// The position itself, normalized against the area with the bottom at
    /// zero. The press seeks straight there.
    AbsoluteVertical,
}

#[derive(bon::Builder)]
pub(crate) struct Scalar {
    track: Track,
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
                state.active = true;
                match self.track {
                    Track::RelativeVertical { value, .. } => {
                        state.start_position = position.y;
                        state.start_value = value;
                        Outcome::captured()
                    }
                    Track::AbsoluteVertical => seek(position, hit.area()),
                }
            }
            Input::PointerMoved if state.active => {
                hit.at()
                    .map_or(Outcome::IGNORED, |position| match self.track {
                        Track::RelativeVertical { range, .. } => Outcome::set(
                            (state.start_value + (state.start_position - position.y) / range)
                                .clamp(0.0, 1.0),
                        ),
                        Track::AbsoluteVertical => seek(position, hit.area()),
                    })
            }
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

fn seek(position: Pt, area: Rect) -> Outcome {
    (area.h > 0.0)
        .then(|| (1.0 - (position.y - area.y) / area.h).clamp(0.0, 1.0))
        .map_or(Outcome::IGNORED, Outcome::set)
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::{super::super::Scroll, *};

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
            .track(Track::RelativeVertical {
                range: 128.0,
                value,
            })
            .hover(Hover::new(CursorShape::ResizeV))
            .build()
    }

    fn resetting(reset: f32) -> Scalar {
        Scalar::builder()
            .track(Track::RelativeVertical {
                range: 128.0,
                value: 0.8,
            })
            .hover(Hover::new(CursorShape::ResizeV))
            .reset(reset)
            .build()
    }

    fn wheel_drag() -> Scalar {
        Scalar::builder()
            .track(Track::RelativeVertical {
                range: 140.0,
                value: 0.5,
            })
            .hover(Hover::new(CursorShape::ResizeV))
            .wheel(WheelStep {
                value: 0.5,
                step: 0.25,
            })
            .build()
    }

    /// The VU meter's area: offset from the origin, so an inverted axis that
    /// forgot to subtract `area.y` would still look right at the top edge.
    fn meter() -> Rect {
        Rect {
            h: 40.0,
            w: 12.0,
            x: 0.0,
            y: 10.0,
        }
    }

    fn on_meter(y: f32) -> Hit {
        Hit::new(Some(Pt { x: 6.0, y }), meter())
    }

    fn seeking() -> Scalar {
        Scalar::builder()
            .track(Track::AbsoluteVertical)
            .hover(Hover::new(CursorShape::ResizeV))
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

    #[kithara::test]
    fn an_absolute_press_seeks_the_inverted_position() {
        let seeking = seeking();
        let now = Instant::now();

        // Every fraction here is dyadic, so the equality holds without a residue.
        for (y, expected) in [(10.0, 1.0), (20.0, 0.75), (30.0, 0.5), (40.0, 0.25)] {
            let mut state = ScalarState::default();
            assert_eq!(
                seeking.on_input(&mut state, Input::PointerDown, &on_meter(y), now),
                Outcome::set(expected),
                "at y={y}"
            );
        }
    }

    #[kithara::test]
    fn an_absolute_drag_on_zero_height_publishes_nothing() {
        let seeking = seeking();
        let mut state = ScalarState::default();
        let now = Instant::now();
        seeking.on_input(&mut state, Input::PointerDown, &on_meter(30.0), now);

        let flattened = Hit::new(Some(Pt { x: 6.0, y: 30.0 }), Rect { h: 0.0, ..meter() });

        assert_eq!(
            seeking.on_input(&mut state, Input::PointerMoved, &flattened, now),
            Outcome::IGNORED,
            "a degenerate height must publish nothing rather than a clamped zero"
        );
    }

    #[kithara::test]
    fn an_absolute_drag_keeps_publishing_after_the_pointer_leaves() {
        let seeking = seeking();
        let mut state = ScalarState::default();
        let now = Instant::now();
        seeking.on_input(&mut state, Input::PointerDown, &on_meter(30.0), now);

        assert_eq!(
            seeking.on_input(&mut state, Input::PointerMoved, &on_meter(200.0), now),
            Outcome::set(0.0)
        );
    }

    #[kithara::test]
    fn an_absolute_press_seeks_and_captures_and_the_release_is_silent() {
        let seeking = seeking();
        let cursor = on_meter(30.0);
        let mut state = ScalarState::default();
        let now = Instant::now();

        let pressed = seeking.on_input(&mut state, Input::PointerDown, &cursor, now);
        let released = seeking.on_input(&mut state, Input::PointerUp, &cursor, now);

        assert_eq!(pressed, Outcome::set(0.5));
        assert_eq!(released, Outcome::captured());
    }

    #[kithara::test]
    fn a_wheel_without_an_opt_in_leaves_the_scroll_to_whoever_is_behind() {
        let seeking = seeking();
        let mut state = ScalarState::default();

        assert_eq!(
            seeking.on_input(
                &mut state,
                Input::Wheel(Scroll::Lines(1.0)),
                &on_meter(30.0),
                Instant::now(),
            ),
            Outcome::IGNORED,
            "a control that did not opt in must let the page scroll"
        );
    }

    #[kithara::test]
    fn without_a_reset_the_second_press_is_an_ordinary_press() {
        let seeking = seeking();
        let cursor = on_meter(30.0);
        let mut state = ScalarState::default();
        let now = Instant::now();

        seeking.on_input(&mut state, Input::PointerDown, &cursor, now);
        seeking.on_input(&mut state, Input::PointerUp, &cursor, now);

        assert_eq!(
            seeking.on_input(&mut state, Input::PointerDown, &cursor, now),
            Outcome::set(0.5),
            "the second press seeks again rather than resetting"
        );
        assert_eq!(
            seeking.on_input(&mut state, Input::PointerMoved, &on_meter(20.0), now),
            Outcome::set(0.75),
            "and it armed the gesture, so the move that follows still drags"
        );
    }
}
