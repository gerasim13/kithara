use kithara_platform::time::Instant;

use super::{
    component::RetainedComponent,
    model::{Descriptor, Emission, Target},
    router::Router,
};
use crate::interact::{CursorShape, Input};

#[derive(Default)]
pub(crate) struct Engine {
    components: Vec<RetainedComponent>,
    router: Router,
}

impl Engine {
    pub(crate) fn reconcile(&mut self, descriptors: impl IntoIterator<Item = Descriptor>) {
        let mut retained = std::mem::take(&mut self.components);
        self.components = descriptors
            .into_iter()
            .map(|descriptor| {
                let retained_index = retained.iter().position(|component| {
                    component.path() == descriptor.path() && component.kind() == descriptor.kind()
                });
                match retained_index {
                    Some(index) => retained.remove(index).reconcile(descriptor),
                    None => descriptor.into(),
                }
            })
            .collect();
        self.router.reconcile(&self.components);
    }

    pub(crate) fn handle(
        &mut self,
        input: Input,
        targets: &[Target<'_>],
        now: Instant,
    ) -> Option<Emission> {
        self.router
            .handle(&mut self.components, input, targets, now)
    }

    pub(crate) fn cursor(&self, targets: &[Target<'_>]) -> CursorShape {
        self.router.cursor(&self.components, targets)
    }

    pub(crate) const fn captures_pointer(&self) -> bool {
        self.router.captures_pointer()
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::{super::model::EngineEvent, *};
    use crate::{
        draw::{Pt, Rect},
        interact::{Hit, Modifiers, Outcome, Scroll},
    };

    fn knob(path: &str, current: f32) -> Descriptor {
        Descriptor::knob(path.to_owned(), current, 100.0, 0.1)
    }

    fn target(path: &str, x: f32, y: f32) -> Target<'_> {
        Target::new(
            path,
            Hit::new(
                Some(Pt { x, y }),
                Rect {
                    h: 100.0,
                    w: 100.0,
                    x: 0.0,
                    y: 0.0,
                },
            ),
        )
    }

    fn value(emission: Option<Emission>) -> Option<f32> {
        emission.and_then(|emission| match emission.outcome.value() {
            Some(EngineEvent::Scalar(value)) => Some(value),
            Some(EngineEvent::Activate) | None => None,
        })
    }

    #[kithara::test]
    fn activation_publishes_once_on_press() {
        let mut engine = Engine::default();
        let path = "gallery/toggles/enabled";
        engine.reconcile([Descriptor::activation(path.to_owned())]);

        let press = engine
            .handle(
                Input::PointerDown,
                &[target(path, 50.0, 50.0)],
                Instant::now(),
            )
            .map(|emission| {
                let captured = emission.outcome.is_captured();
                (emission.outcome.value(), captured)
            });
        assert_eq!(press, Some((Some(EngineEvent::Activate), true)));

        assert!(
            engine
                .handle(
                    Input::PointerMoved {
                        at: Pt { x: 55.0, y: 50.0 },
                    },
                    &[target(path, 55.0, 50.0)],
                    Instant::now(),
                )
                .is_none()
        );
        assert!(
            engine
                .handle(
                    Input::PointerUp,
                    &[target(path, 55.0, 50.0)],
                    Instant::now(),
                )
                .is_none()
        );
    }

    #[kithara::test]
    fn activation_press_that_misses_publishes_nothing() {
        let mut engine = Engine::default();
        let path = "gallery/toggles/enabled";
        engine.reconcile([Descriptor::activation(path.to_owned())]);

        assert!(
            engine
                .handle(
                    Input::PointerDown,
                    &[target(path, 150.0, 50.0)],
                    Instant::now(),
                )
                .is_none()
        );
    }

    #[kithara::test]
    fn activation_does_not_take_the_capture_slot() {
        let mut engine = Engine::default();
        let toggle = "gallery/toggles/enabled";
        let meter = "gallery/meters/level";
        engine.reconcile([
            Descriptor::vertical_vu(meter.to_owned()),
            Descriptor::activation(toggle.to_owned()),
        ]);

        let activation = engine
            .handle(
                Input::PointerDown,
                &[target(toggle, 50.0, 50.0)],
                Instant::now(),
            )
            .map(|emission| emission.outcome.value());
        assert_eq!(activation, Some(Some(EngineEvent::Activate)));
        assert!(
            !engine.captures_pointer(),
            "a captured activation outcome must not occupy the router capture slot"
        );

        let scalar = engine
            .handle(
                Input::PointerDown,
                &[target(meter, 50.0, 25.0)],
                Instant::now(),
            )
            .map(|emission| emission.outcome.value());
        assert_eq!(scalar, Some(Some(EngineEvent::Scalar(0.75))));
    }

    #[kithara::test]
    fn click_wave_seeks_on_press_without_holding_capture() {
        let mut engine = Engine::default();
        let wave = "overview/a/wave";
        let meter = "gallery/meters/level";
        engine.reconcile([
            Descriptor::vertical_vu(meter.to_owned()),
            Descriptor::wave(wave.to_owned()),
        ]);

        let press = engine
            .handle(
                Input::PointerDown,
                &[target(wave, 25.0, 50.0)],
                Instant::now(),
            )
            .map(|emission| {
                let captured = emission.outcome.is_captured();
                (emission.path, emission.outcome.value(), captured)
            });
        assert_eq!(
            press,
            Some((wave.to_owned(), Some(EngineEvent::Scalar(0.25)), true,))
        );
        assert!(
            !engine.captures_pointer(),
            "a click wave answers the press without retaining the pointer"
        );

        let next = engine
            .handle(
                Input::PointerDown,
                &[target(meter, 50.0, 25.0)],
                Instant::now(),
            )
            .map(|emission| (emission.path, emission.outcome.value()));
        assert_eq!(
            next,
            Some((meter.to_owned(), Some(EngineEvent::Scalar(0.75)),))
        );
    }

    #[kithara::test]
    fn hero_wave_shift_drag_publishes_child_endpoints_and_releases_capture() {
        let mut engine = Engine::default();
        let path = "deck-a/wave";
        let now = Instant::now();
        engine.reconcile([Descriptor::hero_wave(
            path.to_owned(),
            0.5,
            0.5,
            0.25..0.75,
            0.625,
            0.4,
        )]);

        assert!(
            engine
                .handle(
                    Input::ModifiersChanged(Modifiers::new(true)),
                    &[target(path, 25.0, 50.0)],
                    now,
                )
                .is_none()
        );
        let start = engine
            .handle(Input::PointerDown, &[target(path, 25.0, 50.0)], now)
            .unwrap_or_else(|| panic!("a shifted press must publish the loop start"));
        assert_eq!(start.child, Some("loop_start"));
        assert_eq!(start.outcome.value(), Some(EngineEvent::Scalar(0.375)));
        assert!(engine.captures_pointer());

        let end = engine
            .handle(
                Input::PointerMoved {
                    at: Pt { x: 75.0, y: 50.0 },
                },
                &[target(path, 75.0, 50.0)],
                now,
            )
            .unwrap_or_else(|| panic!("a shifted drag must publish the loop end"));
        assert_eq!(end.child, Some("loop_end"));
        assert_eq!(end.outcome.value(), Some(EngineEvent::Scalar(0.625)));
        assert!(engine.captures_pointer());

        let release = engine
            .handle(Input::PointerUp, &[target(path, 75.0, 50.0)], now)
            .unwrap_or_else(|| panic!("the loop release must finish the gesture"));
        assert_eq!(release.child, None);
        assert_eq!(release.outcome, Outcome::captured());
        assert!(!engine.captures_pointer());
    }

    #[kithara::test]
    fn hero_wave_refreshes_its_plain_drag_and_keeps_grip_outside_bounds() {
        let mut engine = Engine::default();
        let wave = "deck-a/wave";
        let meter = "gallery/meters/level";
        engine.reconcile([
            Descriptor::vertical_vu(meter.to_owned()),
            Descriptor::hero_wave(wave.to_owned(), 0.5, 0.75, 0.5..1.0, 0.5, 0.4),
        ]);

        let press = engine
            .handle(
                Input::PointerDown,
                &[target(wave, 50.0, 50.0)],
                Instant::now(),
            )
            .map(|emission| (emission.path, emission.outcome));
        assert_eq!(press, Some((wave.to_owned(), Outcome::captured())));
        assert!(engine.captures_pointer());

        engine.reconcile([
            Descriptor::vertical_vu(meter.to_owned()),
            Descriptor::hero_wave(wave.to_owned(), 0.25, 0.0, 0.0..0.25, 0.3, 0.2),
        ]);
        let moved = engine
            .handle(
                Input::PointerMoved {
                    at: Pt { x: 150.0, y: 50.0 },
                },
                &[target(wave, 150.0, 50.0), target(meter, 50.0, 25.0)],
                Instant::now(),
            )
            .map(|emission| (emission.path, emission.outcome.value()));
        assert_eq!(
            moved,
            Some((wave.to_owned(), Some(EngineEvent::Scalar(0.5))))
        );
    }

    #[kithara::test]
    fn hero_wave_wheel_publishes_zoom_without_holding_capture() {
        let mut engine = Engine::default();
        let path = "deck-a/wave";
        engine.reconcile([Descriptor::hero_wave(
            path.to_owned(),
            0.5,
            0.5,
            0.25..0.75,
            0.625,
            0.4,
        )]);

        for (delta, expected) in [(1.0, 0.625), (-1.0, 0.4), (0.0, 0.4)] {
            let emission = engine
                .handle(
                    Input::Wheel(Scroll::Lines(delta)),
                    &[target(path, 50.0, 50.0)],
                    Instant::now(),
                )
                .unwrap_or_else(|| panic!("a hero wave wheel must publish zoom"));
            assert_eq!(emission.child, Some("zoom"));
            assert_eq!(
                emission.outcome.value(),
                Some(EngineEvent::Scalar(expected))
            );
            assert!(!engine.captures_pointer());
        }
    }

    #[kithara::test]
    fn changing_wave_style_rebuilds_state_and_clears_hero_capture() {
        let mut engine = Engine::default();
        let path = "deck-a/wave";
        let now = Instant::now();
        engine.reconcile([Descriptor::hero_wave(
            path.to_owned(),
            0.5,
            0.5,
            0.25..0.75,
            0.625,
            0.4,
        )]);
        engine.handle(Input::PointerDown, &[target(path, 50.0, 50.0)], now);
        assert!(engine.captures_pointer());

        engine.reconcile([Descriptor::wave(path.to_owned())]);

        assert!(!engine.captures_pointer());
        assert!(
            engine
                .handle(
                    Input::PointerMoved {
                        at: Pt { x: 75.0, y: 50.0 },
                    },
                    &[target(path, 75.0, 50.0)],
                    now,
                )
                .is_none(),
            "ordinary Wave must not retain HeroWave's plain-drag state"
        );
    }

    #[kithara::test]
    fn reconciliation_refreshes_config_and_retains_an_active_drag() {
        let mut engine = Engine::default();
        let now = Instant::now();
        engine.reconcile([knob("studio/gain", 0.25)]);

        let press = engine.handle(
            Input::PointerDown,
            &[target("studio/gain", 50.0, 50.0)],
            now,
        );
        assert_eq!(
            press.map(|emission| emission.outcome),
            Some(Outcome::captured())
        );

        engine.reconcile([Descriptor::knob("studio/gain".to_owned(), 0.9, 200.0, 0.2)]);
        assert_eq!(
            value(engine.handle(
                Input::PointerMoved {
                    at: Pt { x: 50.0, y: 0.0 },
                },
                &[target("studio/gain", 50.0, 0.0)],
                now,
            )),
            Some(0.5),
            "the retained start value combines with the refreshed drag range"
        );
    }

    #[kithara::test]
    fn a_kind_change_rebuilds_state_and_clears_the_captured_identity() {
        let mut engine = Engine::default();
        let now = Instant::now();
        engine.reconcile([knob("studio/level", 0.5)]);
        engine.handle(
            Input::PointerDown,
            &[target("studio/level", 50.0, 50.0)],
            now,
        );

        engine.reconcile([Descriptor::vertical_vu("studio/level".to_owned())]);
        let emission = engine
            .handle(
                Input::PointerDown,
                &[target("studio/level", 50.0, 25.0)],
                now,
            )
            .map(|emission| {
                let value = match emission.outcome.value() {
                    Some(EngineEvent::Scalar(value)) => Some(value),
                    Some(EngineEvent::Activate) | None => None,
                };
                (emission.path, value)
            });

        assert_eq!(emission, Some(("studio/level".to_owned(), Some(0.75))));
    }

    #[kithara::test]
    fn topmost_non_ignored_component_handles_input_first() {
        let mut engine = Engine::default();
        engine.reconcile([
            knob("studio/back", 0.5),
            Descriptor::vertical_vu("studio/front".to_owned()),
        ]);

        let emission = engine
            .handle(
                Input::PointerDown,
                &[
                    target("studio/back", 50.0, 25.0),
                    target("studio/front", 50.0, 25.0),
                ],
                Instant::now(),
            )
            .map(|emission| emission.path);

        assert_eq!(emission.as_deref(), Some("studio/front"));
    }

    #[kithara::test]
    fn capture_holder_routes_exclusively_until_release() {
        let mut engine = Engine::default();
        let now = Instant::now();
        engine.reconcile([
            knob("studio/back", 0.5),
            Descriptor::vertical_vu("studio/front".to_owned()),
        ]);
        engine.handle(
            Input::PointerDown,
            &[
                target("studio/back", 50.0, 25.0),
                target("studio/front", 50.0, 25.0),
            ],
            now,
        );

        let moved = engine
            .handle(
                Input::PointerMoved {
                    at: Pt { x: 50.0, y: 125.0 },
                },
                &[
                    target("studio/front", 50.0, 125.0),
                    target("studio/back", 50.0, 50.0),
                ],
                now,
            )
            .map(|emission| emission.path);
        assert_eq!(moved.as_deref(), Some("studio/front"));

        engine.handle(
            Input::PointerUp,
            &[target("studio/front", 50.0, 125.0)],
            now,
        );
        assert!(!engine.captures_pointer());
        let next = engine
            .handle(
                Input::PointerDown,
                &[
                    target("studio/front", 50.0, 50.0),
                    target("studio/back", 50.0, 50.0),
                ],
                now,
            )
            .map(|emission| emission.path);
        assert_eq!(next.as_deref(), Some("studio/back"));
    }

    #[kithara::test]
    fn captured_outcome_does_not_persist_capture_without_active_state() {
        let mut engine = Engine::default();
        let now = Instant::now();
        engine.reconcile([knob("studio/back", 0.5), knob("studio/front", 0.5)]);

        let wheel = engine.handle(
            Input::Wheel(Scroll::Lines(0.0)),
            &[
                target("studio/back", 50.0, 50.0),
                target("studio/front", 50.0, 50.0),
            ],
            now,
        );
        assert_eq!(
            wheel.map(|emission| emission.outcome),
            Some(Outcome::captured())
        );
        assert!(
            !engine.captures_pointer(),
            "a wheel outcome must not occupy the router capture slot"
        );

        let next = engine
            .handle(
                Input::PointerDown,
                &[
                    target("studio/front", 50.0, 50.0),
                    target("studio/back", 50.0, 50.0),
                ],
                now,
            )
            .map(|emission| emission.path);
        assert_eq!(next.as_deref(), Some("studio/back"));
    }

    #[kithara::test]
    fn cursor_follows_active_capture_then_topmost_hover() {
        let mut engine = Engine::default();
        let now = Instant::now();
        engine.reconcile([knob("studio/back", 0.5), knob("studio/front", 0.5)]);

        assert_eq!(
            engine.cursor(&[
                target("studio/back", 50.0, 50.0),
                target("studio/front", 150.0, 150.0),
            ]),
            CursorShape::ResizeV
        );
        engine.handle(
            Input::PointerDown,
            &[target("studio/front", 50.0, 50.0)],
            now,
        );
        assert_eq!(
            engine.cursor(&[target("studio/front", 150.0, 150.0)]),
            CursorShape::ResizeV
        );
        engine.handle(
            Input::PointerUp,
            &[target("studio/front", 150.0, 150.0)],
            now,
        );
        assert_eq!(
            engine.cursor(&[
                target("studio/back", 150.0, 150.0),
                target("studio/front", 150.0, 150.0),
            ]),
            CursorShape::None
        );
    }

    #[kithara::test]
    fn stereo_meter_seeks_horizontally_with_a_horizontal_cursor() {
        let mut engine = Engine::default();
        let path = "gallery/meters/stereo";
        engine.reconcile([Descriptor::stereo_meter(path.to_owned())]);

        assert_eq!(
            engine.cursor(&[target(path, 25.0, 50.0)]),
            CursorShape::ResizeH
        );
        assert_eq!(
            value(engine.handle(
                Input::PointerDown,
                &[target(path, 25.0, 50.0)],
                Instant::now(),
            )),
            Some(0.25)
        );
    }

    #[kithara::test]
    fn crossfader_seeks_horizontally_with_a_horizontal_cursor() {
        let mut engine = Engine::default();
        let path = "mixer/xfade";
        engine.reconcile([Descriptor::crossfader(path.to_owned())]);

        assert_eq!(
            engine.cursor(&[target(path, 25.0, 50.0)]),
            CursorShape::ResizeH
        );
        assert_eq!(
            value(engine.handle(
                Input::PointerDown,
                &[target(path, 25.0, 50.0)],
                Instant::now(),
            )),
            Some(0.25)
        );
    }
}
