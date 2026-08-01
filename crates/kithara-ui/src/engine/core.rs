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
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        draw::{Pt, Rect},
        interact::{Hit, Outcome, Scroll},
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
        emission.and_then(|emission| emission.outcome.value())
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
            .map(|emission| (emission.path, emission.outcome.value()));

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
}
