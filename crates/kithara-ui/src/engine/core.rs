use kithara_platform::time::Instant;

use super::{
    component::RetainedComponent,
    model::{Descriptor, Emission, Kind, Target},
    router::Router,
};
use crate::{
    draw::Rect,
    interact::{CursorShape, Input},
};

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

    pub(crate) fn scroll_offset(&self, path: &str) -> Option<f32> {
        self.components
            .iter()
            .find(|component| component.path() == path && component.kind() == Kind::Scroll)
            .and_then(RetainedComponent::scroll_offset)
    }

    pub(crate) fn pressed_item_index(&self, path: &str) -> Option<usize> {
        self.components
            .iter()
            .find(|component| component.kind() == Kind::Item && component.event_path() == path)
            .and_then(RetainedComponent::pressed_item_index)
    }

    pub(crate) fn has_pressed_item(&self) -> bool {
        self.components
            .iter()
            .any(|component| component.pressed_item_index().is_some())
    }

    pub(crate) fn set_scroll_viewport(&mut self, path: &str, area: Rect) {
        if let Some(component) = self
            .components
            .iter_mut()
            .find(|component| component.path() == path && component.kind() == Kind::Scroll)
        {
            component.set_scroll_viewport(area);
        }
    }

    pub(crate) const fn captures_pointer(&self) -> bool {
        self.router.captures_pointer()
    }

    pub(crate) fn captures(&self, path: &str) -> bool {
        self.router.captures(path)
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::{super::model::EngineEvent, *};
    use crate::{
        draw::{Pt, Rect},
        engine::ScrollConfig,
        interact::{
            Hit, Hover, Modifiers, Outcome, Scroll, ScrollAxis,
            recognizers::{DragEvent, WheelStep},
        },
    };

    fn knob(path: &str, current: f32) -> Descriptor {
        Descriptor::knob(path.to_owned(), current, 100.0, 0.1)
    }

    fn scroll(path: &str) -> Descriptor {
        Descriptor::scroll(
            path.to_owned(),
            ScrollConfig::items(ScrollAxis::Vertical, 200.0, 10, 20.0, 20.0, 8.0),
        )
    }

    fn plain_scroll(path: &str, axis: ScrollAxis) -> Descriptor {
        Descriptor::scroll(path.to_owned(), ScrollConfig::plain(axis, 200.0))
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

    fn item_target(path: &str, index: usize, x: f32, y: f32) -> Target<'_> {
        let target = target(path, x, y);
        Target::item(path, target.hit, index)
    }

    fn axis_lines(axis: ScrollAxis, delta: f32) -> Scroll {
        match axis {
            ScrollAxis::Horizontal => Scroll::Lines { x: delta, y: 0.0 },
            ScrollAxis::Vertical => Scroll::Lines { x: 0.0, y: delta },
        }
    }

    fn axis_pixels(axis: ScrollAxis, delta: f32) -> Scroll {
        match axis {
            ScrollAxis::Horizontal => Scroll::Pixels { x: delta, y: 0.0 },
            ScrollAxis::Vertical => Scroll::Pixels { x: 0.0, y: delta },
        }
    }

    fn value(emission: Option<Emission>) -> Option<f64> {
        emission.and_then(|emission| match emission.outcome.value() {
            Some(EngineEvent::Scalar(value)) => Some(value),
            Some(
                EngineEvent::Activate
                | EngineEvent::Crossing(_)
                | EngineEvent::Index(_)
                | EngineEvent::Drag { .. },
            )
            | None => None,
        })
    }

    #[kithara::test]
    fn fader_quantizes_the_wide_value_at_iced_step_boundaries() {
        let path = "gallery/faders/default";
        let observed = [29.0, 30.0, 31.0].map(|x| {
            let mut engine = Engine::default();
            engine.reconcile([Descriptor::fader(
                path.to_owned(),
                Hover::new(CursorShape::Grab),
                Some(0.01),
                None,
            )]);
            value(engine.handle(
                Input::PointerDown,
                &[Target::new(
                    path,
                    Hit::new(
                        Some(Pt { x, y: 8.0 }),
                        Rect {
                            h: 16.0,
                            w: 200.0,
                            x: 0.0,
                            y: 0.0,
                        },
                    ),
                )],
                Instant::now(),
            ))
        });

        assert_eq!(observed, [Some(0.14), Some(0.15), Some(0.16)]);
    }

    #[kithara::test]
    fn volume_fader_keeps_drag_continuous_and_wheel_step_separate() {
        let path = "gallery/faders/volume";
        let mut engine = Engine::default();
        engine.reconcile([Descriptor::fader(
            path.to_owned(),
            Hover::new(CursorShape::ResizeH),
            None,
            Some(WheelStep {
                value: 0.5,
                step: 0.01,
            }),
        )]);
        let targets = [Target::new(
            path,
            Hit::new(
                Some(Pt { x: 29.0, y: 7.0 }),
                Rect {
                    h: 14.0,
                    w: 200.0,
                    x: 0.0,
                    y: 0.0,
                },
            ),
        )];

        assert_eq!(
            value(engine.handle(Input::PointerDown, &targets, Instant::now())),
            Some(f64::from(29.0_f32 / 200.0_f32))
        );
        assert_eq!(
            value(engine.handle(Input::Wheel(Scroll::lines(1.0)), &targets, Instant::now(),)),
            Some(f64::from(0.49_f32))
        );
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
    fn crossing_publishes_once_per_boundary_and_survives_reconciliation() {
        let mut engine = Engine::default();
        let path = "deck-a/drop";
        engine.reconcile([Descriptor::crossing(path.to_owned())]);

        let enter = engine
            .handle(
                Input::PointerMoved {
                    at: Pt { x: 50.0, y: 50.0 },
                },
                &[target(path, 50.0, 50.0)],
                Instant::now(),
            )
            .unwrap_or_else(|| panic!("entering the drop zone must publish"));
        assert_eq!(enter.path, path);
        assert_eq!(enter.child, None);
        assert_eq!(enter.outcome.value(), Some(EngineEvent::Crossing(true)));
        assert!(!enter.outcome.is_captured());
        assert!(!engine.captures_pointer());

        engine.reconcile([Descriptor::crossing(path.to_owned())]);
        assert!(
            engine
                .handle(
                    Input::PointerMoved {
                        at: Pt { x: 55.0, y: 50.0 },
                    },
                    &[target(path, 55.0, 50.0)],
                    Instant::now(),
                )
                .is_none(),
            "reconciliation must not turn an inside move into another entry"
        );

        let leave = engine
            .handle(
                Input::PointerLeft,
                &[target(path, 55.0, 50.0)],
                Instant::now(),
            )
            .unwrap_or_else(|| panic!("leaving the drop zone must publish"));
        assert_eq!(leave.outcome.value(), Some(EngineEvent::Crossing(false)));
        assert!(!leave.outcome.is_captured());
        assert!(!engine.captures_pointer());
        assert!(
            engine
                .handle(
                    Input::PointerLeft,
                    &[target(path, 55.0, 50.0)],
                    Instant::now(),
                )
                .is_none()
        );
    }

    #[kithara::test]
    fn segmented_publishes_the_uniform_cell_index_on_press() {
        let mut engine = Engine::default();
        let path = "cells/beat";
        let area = Rect {
            h: 20.0,
            w: 100.0,
            x: 10.0,
            y: 20.0,
        };
        engine.reconcile([Descriptor::segmented(path.to_owned(), 4)]);

        assert_eq!(
            engine.cursor(&[Target::new(
                path,
                Hit::new(Some(Pt { x: 10.0, y: 30.0 }), area),
            )]),
            CursorShape::Pointer
        );

        for (x, expected) in [(10.0, 0), (34.0, 0), (35.0, 1), (109.0, 3)] {
            let emission = engine
                .handle(
                    Input::PointerDown,
                    &[Target::new(path, Hit::new(Some(Pt { x, y: 30.0 }), area))],
                    Instant::now(),
                )
                .unwrap_or_else(|| panic!("a segmented press must publish its cell index"));
            assert_eq!(emission.outcome.value(), Some(EngineEvent::Index(expected)));
            assert!(!engine.captures_pointer());
        }

        assert!(
            engine
                .handle(
                    Input::PointerDown,
                    &[Target::new(
                        path,
                        Hit::new(Some(Pt { x: 110.0, y: 30.0 }), area),
                    )],
                    Instant::now(),
                )
                .is_none()
        );
    }

    #[kithara::test]
    fn retained_item_drag_keeps_its_target_identity_and_publishes_the_row_index() {
        let mut engine = Engine::default();
        let path = "library/tracks";
        let target_path = "library/tracks/rows";
        let now = Instant::now();
        engine.reconcile([Descriptor::item(target_path.to_owned(), path.to_owned(), 4)]);

        assert_eq!(
            engine
                .handle(
                    Input::PointerDown,
                    &[item_target(target_path, 3, 10.0, 13.0)],
                    now,
                )
                .map(|emission| emission.outcome),
            Some(Outcome::captured())
        );
        assert_eq!(engine.pressed_item_index(path), Some(3));
        assert!(
            engine
                .handle(
                    Input::PointerMoved {
                        at: Pt { x: 11.0, y: 13.0 },
                    },
                    &[item_target(target_path, 3, 11.0, 13.0)],
                    now,
                )
                .is_none()
        );
        let started = engine
            .handle(
                Input::PointerMoved {
                    at: Pt { x: 40.0, y: 13.0 },
                },
                &[item_target(target_path, 3, 40.0, 13.0)],
                now,
            )
            .unwrap_or_else(|| panic!("crossing the threshold must start the row drag"));

        assert_eq!(started.path, path);
        assert_eq!(
            started.outcome,
            Outcome::observed(EngineEvent::Drag {
                event: DragEvent::Started,
                index: 3,
            })
        );
        assert!(!engine.captures_pointer());

        let dropped = engine
            .handle(Input::PointerUp, &[target(target_path, 200.0, 200.0)], now)
            .unwrap_or_else(|| panic!("the row watcher must publish after its hit leaves view"));
        assert_eq!(
            dropped.outcome,
            Outcome::observed(EngineEvent::Drag {
                event: DragEvent::Dropped,
                index: 3,
            })
        );
        assert_eq!(engine.pressed_item_index(path), None);
    }

    #[kithara::test]
    fn retained_item_selects_on_release_without_starting_a_drag() {
        let mut engine = Engine::default();
        let path = "library/tracks";
        let target_path = "library/tracks/rows";
        let now = Instant::now();
        engine.reconcile([Descriptor::item(target_path.to_owned(), path.to_owned(), 4)]);

        let _ = engine.handle(
            Input::PointerDown,
            &[item_target(target_path, 2, 10.0, 13.0)],
            now,
        );
        let selected = engine
            .handle(
                Input::PointerUp,
                &[item_target(target_path, 2, 10.0, 13.0)],
                now,
            )
            .unwrap_or_else(|| panic!("a plain row release must publish its index"));

        assert_eq!(selected.path, path);
        assert_eq!(selected.outcome, Outcome::set(EngineEvent::Index(2)));
        assert_eq!(engine.pressed_item_index(path), None);
    }

    #[kithara::test]
    fn retained_item_cancels_a_held_index_removed_by_reconciliation() {
        let mut engine = Engine::default();
        let path = "library/tracks";
        let target_path = "library/tracks/rows";
        let now = Instant::now();
        engine.reconcile([Descriptor::item(target_path.to_owned(), path.to_owned(), 4)]);
        let _ = engine.handle(
            Input::PointerDown,
            &[item_target(target_path, 3, 10.0, 13.0)],
            now,
        );

        engine.reconcile([Descriptor::item(target_path.to_owned(), path.to_owned(), 3)]);
        assert_eq!(engine.pressed_item_index(path), None);
        assert!(
            engine
                .handle(
                    Input::PointerMoved {
                        at: Pt { x: 40.0, y: 13.0 },
                    },
                    &[Target::new(
                        target_path,
                        Hit::new(
                            None,
                            Rect {
                                h: 0.0,
                                w: 0.0,
                                x: 0.0,
                                y: 0.0,
                            },
                        ),
                    )],
                    now,
                )
                .is_none()
        );
    }

    #[kithara::test]
    fn column_divider_uses_the_wide_hit_rect_and_publishes_pixel_width() {
        let mut engine = Engine::default();
        let path = "library/tracks/width/deck";
        let now = Instant::now();
        let hit_rect = Rect {
            h: 22.0,
            w: 7.0,
            x: 100.0,
            y: 0.0,
        };
        let divider = Target::new(path, Hit::new(Some(Pt { x: 100.5, y: 11.0 }), hit_rect));
        engine.reconcile([Descriptor::column_divider(path.to_owned(), 64.0, 28.0)]);

        assert_eq!(divider.hit.area(), hit_rect);
        assert_eq!(
            engine
                .handle(Input::PointerDown, &[divider], now)
                .map(|emission| emission.outcome),
            Some(Outcome::captured()),
            "the outer half-pixel of the seven-pixel grab area must arm the drag"
        );
        assert_eq!(
            value(engine.handle(
                Input::PointerMoved {
                    at: Pt { x: 140.5, y: 11.0 },
                },
                &[divider],
                now,
            )),
            Some(104.0)
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

        for (delta, expected) in [(1.0, 0.625_f32), (-1.0, 0.4), (0.0, 0.4)] {
            let emission = engine
                .handle(
                    Input::Wheel(Scroll::lines(delta)),
                    &[target(path, 50.0, 50.0)],
                    Instant::now(),
                )
                .unwrap_or_else(|| panic!("a hero wave wheel must publish zoom"));
            assert_eq!(emission.child, Some("zoom"));
            assert_eq!(
                emission.outcome.value(),
                Some(EngineEvent::Scalar(f64::from(expected)))
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
    fn fader_reconciliation_refreshes_the_wide_drag_step_without_losing_capture() {
        let mut engine = Engine::default();
        let path = "gallery/faders/default";
        let now = Instant::now();
        engine.reconcile([Descriptor::fader(
            path.to_owned(),
            Hover::new(CursorShape::Grab),
            Some(0.01),
            None,
        )]);
        let _ = engine.handle(Input::PointerDown, &[target(path, 29.0, 50.0)], now);
        assert!(engine.captures_pointer());

        engine.reconcile([Descriptor::fader(
            path.to_owned(),
            Hover::new(CursorShape::Grab),
            Some(0.2),
            None,
        )]);

        assert!(engine.captures_pointer());
        assert_eq!(
            value(engine.handle(
                Input::PointerMoved {
                    at: Pt { x: 31.0, y: 50.0 },
                },
                &[target(path, 31.0, 50.0)],
                now,
            )),
            Some(0.4)
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
                    Some(
                        EngineEvent::Activate
                        | EngineEvent::Crossing(_)
                        | EngineEvent::Index(_)
                        | EngineEvent::Drag { .. },
                    )
                    | None => None,
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
            Input::Wheel(Scroll::lines(0.0)),
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
    fn innermost_scroll_that_can_move_consumes_the_wheel() {
        let mut engine = Engine::default();
        let now = Instant::now();
        engine.reconcile([scroll("outer"), scroll("inner")]);

        let emission = engine
            .handle(
                Input::Wheel(Scroll::lines(-1.0)),
                &[target("outer", 50.0, 50.0), target("inner", 50.0, 50.0)],
                now,
            )
            .unwrap_or_else(|| panic!("the innermost movable scroll must consume the wheel"));

        assert_eq!(emission.path, "inner");
        assert_eq!(emission.outcome, Outcome::captured());
        assert_eq!(engine.scroll_offset("inner"), Some(60.0));
        assert_eq!(engine.scroll_offset("outer"), Some(0.0));
    }

    #[kithara::test]
    fn vertical_scroll_passes_a_horizontal_wheel_to_the_outer_horizontal_scroll() {
        let mut engine = Engine::default();
        let now = Instant::now();
        engine.reconcile([
            plain_scroll("table/scroll-x", ScrollAxis::Horizontal),
            plain_scroll("table", ScrollAxis::Vertical),
        ]);

        let emission = engine
            .handle(
                Input::Wheel(Scroll::Lines { x: -1.0, y: 0.0 }),
                &[
                    target("table/scroll-x", 50.0, 50.0),
                    target("table", 50.0, 50.0),
                ],
                now,
            )
            .unwrap_or_else(|| panic!("the outer horizontal scroll must consume the wheel"));

        assert_eq!(emission.path, "table/scroll-x");
        assert_eq!(emission.outcome, Outcome::captured());
        assert_eq!(engine.scroll_offset("table"), Some(0.0));
        assert_eq!(engine.scroll_offset("table/scroll-x"), Some(60.0));
    }

    #[kithara::test]
    fn each_scroll_axis_consumes_both_directions_only_while_it_can_move() {
        for axis in [ScrollAxis::Horizontal, ScrollAxis::Vertical] {
            let mut engine = Engine::default();
            let now = Instant::now();
            let path = match axis {
                ScrollAxis::Horizontal => "horizontal",
                ScrollAxis::Vertical => "vertical",
            };
            let target = target(path, 50.0, 50.0);
            engine.reconcile([plain_scroll(path, axis)]);

            assert!(
                engine
                    .handle(Input::Wheel(axis_lines(axis, 1.0)), &[target], now)
                    .is_none(),
                "a wheel past the leading boundary must remain ignored for {axis:?}"
            );
            let toward_end = engine
                .handle(Input::Wheel(axis_lines(axis, -1.0)), &[target], now)
                .unwrap_or_else(|| panic!("{axis:?} must consume travel toward its end"));
            assert_eq!(toward_end.outcome, Outcome::captured());

            let _ = engine.handle(Input::Wheel(axis_pixels(axis, -1_000.0)), &[target], now);
            assert_eq!(engine.scroll_offset(path), Some(100.0));
            assert!(
                engine
                    .handle(Input::Wheel(axis_pixels(axis, -1.0)), &[target], now)
                    .is_none(),
                "a wheel past the trailing boundary must remain ignored for {axis:?}"
            );
            let toward_start = engine
                .handle(Input::Wheel(axis_lines(axis, 1.0)), &[target], now)
                .unwrap_or_else(|| panic!("{axis:?} must consume travel toward its start"));
            assert_eq!(toward_start.outcome, Outcome::captured());
            assert_eq!(engine.scroll_offset(path), Some(40.0));
        }
    }

    #[kithara::test]
    fn scroll_outside_the_hit_is_ignored() {
        let mut engine = Engine::default();
        let path = "tree/browser";
        engine.reconcile([scroll(path)]);

        assert!(
            engine
                .handle(
                    Input::Wheel(Scroll::lines(-1.0)),
                    &[target(path, 150.0, 150.0)],
                    Instant::now(),
                )
                .is_none()
        );
        assert_eq!(engine.scroll_offset(path), Some(0.0));
    }

    #[kithara::test]
    fn wheel_continues_outward_when_the_inner_scroll_is_at_its_boundary() {
        let mut engine = Engine::default();
        let now = Instant::now();
        engine.reconcile([scroll("outer"), scroll("inner")]);
        let inner = target("inner", 50.0, 50.0);

        let _ = engine.handle(Input::Wheel(Scroll::pixels(-1_000.0)), &[inner], now);
        assert_eq!(engine.scroll_offset("inner"), Some(100.0));

        let emission = engine
            .handle(
                Input::Wheel(Scroll::pixels(-10.0)),
                &[target("outer", 50.0, 50.0), inner],
                now,
            )
            .unwrap_or_else(|| panic!("the movable outer scroll must receive the wheel"));

        assert_eq!(emission.path, "outer");
        assert_eq!(emission.outcome, Outcome::captured());
        assert_eq!(engine.scroll_offset("inner"), Some(100.0));
        assert_eq!(engine.scroll_offset("outer"), Some(10.0));
    }

    #[kithara::test]
    fn bottom_scroll_ignores_downward_wheel_but_consumes_upward_wheel() {
        let mut engine = Engine::default();
        let now = Instant::now();
        let path = "tree/browser";
        let target = target(path, 50.0, 50.0);
        engine.reconcile([scroll(path)]);

        let down = engine
            .handle(Input::Wheel(Scroll::pixels(-1_000.0)), &[target], now)
            .unwrap_or_else(|| panic!("the scroll must consume travel to the bottom"));
        assert_eq!(down.outcome, Outcome::captured());
        assert_eq!(engine.scroll_offset(path), Some(100.0));

        assert!(
            engine
                .handle(Input::Wheel(Scroll::pixels(-1.0)), &[target], now)
                .is_none(),
            "a downward wheel at the bottom must remain ignored"
        );

        let up = engine
            .handle(Input::Wheel(Scroll::lines(1.0)), &[target], now)
            .unwrap_or_else(|| panic!("an upward wheel at the bottom must be consumed"));
        assert_eq!(up.outcome, Outcome::captured());
        assert_eq!(engine.scroll_offset(path), Some(40.0));
    }

    #[kithara::test]
    fn scrollbar_lane_does_not_activate_the_row_behind_it() {
        let mut engine = Engine::default();
        let path = "tree/browser";
        engine.reconcile([scroll(path)]);

        assert!(
            engine
                .handle(
                    Input::PointerDown,
                    &[target(path, 99.0, 10.0)],
                    Instant::now(),
                )
                .is_none()
        );
    }

    #[kithara::test]
    fn layout_viewport_clamps_the_canonical_offset() {
        let mut engine = Engine::default();
        let path = "tree/browser";
        let target = target(path, 50.0, 50.0);
        engine.reconcile([scroll(path)]);

        let _ = engine.handle(
            Input::Wheel(Scroll::pixels(-1_000.0)),
            &[target],
            Instant::now(),
        );
        assert_eq!(engine.scroll_offset(path), Some(100.0));

        engine.set_scroll_viewport(
            path,
            Rect {
                h: 180.0,
                w: 100.0,
                x: 0.0,
                y: 0.0,
            },
        );

        assert_eq!(engine.scroll_offset(path), Some(20.0));
    }

    #[kithara::test]
    fn scroll_offset_survives_descriptor_reconciliation() {
        let mut engine = Engine::default();
        let path = "tree/browser";
        let target = target(path, 50.0, 50.0);
        engine.reconcile([scroll(path)]);
        let _ = engine.handle(Input::Wheel(Scroll::lines(-1.0)), &[target], Instant::now());

        engine.reconcile([scroll(path)]);

        assert_eq!(engine.scroll_offset(path), Some(60.0));
    }

    #[kithara::test]
    fn scrolled_row_click_emits_the_visible_index() {
        let mut engine = Engine::default();
        let now = Instant::now();
        let path = "tree/browser";
        engine.reconcile([scroll(path)]);
        let target = target(path, 50.0, 10.0);

        let wheel = engine
            .handle(Input::Wheel(Scroll::lines(-1.0)), &[target], now)
            .unwrap_or_else(|| panic!("the tree must scroll before the row click"));
        assert_eq!(wheel.outcome, Outcome::captured());
        assert_eq!(wheel.outcome.value(), None);

        let click = engine
            .handle(Input::PointerDown, &[target], now)
            .unwrap_or_else(|| panic!("the visible row must activate"));
        assert_eq!(click.outcome, Outcome::set(EngineEvent::Index(3)));
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
