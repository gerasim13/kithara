use std::rc::Rc;

use iced::{
    Event, Rectangle, Renderer, Theme,
    mouse::{Cursor, Interaction},
    widget::canvas::{self, Action, Frame, Geometry},
};
use kithara_platform::time::Instant;

use super::{paint::PickerPaint, picker_hits, widget::PickerState};
use crate::{
    backends::IcedBackend,
    draw::{Rect, replay},
    engine::Target,
    interact::iced as iced_interact,
    render::{UiEvent, engine as engine_event},
};

pub(super) struct InputProgram<'a> {
    paint: Rc<PickerPaint<'a>>,
    path: String,
}

impl<'a> InputProgram<'a> {
    pub(super) fn new(path: &str, paint: Rc<PickerPaint<'a>>) -> Self {
        Self {
            paint,
            path: path.to_owned(),
        }
    }
}

impl canvas::Program<UiEvent> for InputProgram<'_> {
    type State = PickerState;

    fn update(
        &self,
        state: &mut PickerState,
        event: &Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Option<Action<UiEvent>> {
        let input = iced_interact::input(event)?;
        let targets = targets(
            &self.path,
            bounds,
            cursor,
            state.snapshot().open,
            self.paint.item_count(),
            self.paint.item_height(),
        );
        let emission = state.engine_mut()?.handle(input, &targets, Instant::now());
        state.refresh(&self.path);
        emission.and_then(|emission| engine_event(&emission.path, emission.child, emission.outcome))
    }

    fn draw(
        &self,
        state: &PickerState,
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Vec<Geometry> {
        geometry(&self.paint, state, renderer, theme, bounds, cursor)
    }

    fn mouse_interaction(
        &self,
        _state: &PickerState,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Interaction {
        if cursor.is_over(bounds) {
            Interaction::Pointer
        } else {
            Interaction::None
        }
    }
}

pub(super) struct PaintProgram<'a> {
    paint: Rc<PickerPaint<'a>>,
}

impl<'a> PaintProgram<'a> {
    pub(super) const fn new(paint: Rc<PickerPaint<'a>>) -> Self {
        Self { paint }
    }
}

impl canvas::Program<UiEvent> for PaintProgram<'_> {
    type State = PickerState;

    fn draw(
        &self,
        state: &PickerState,
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Vec<Geometry> {
        geometry(&self.paint, state, renderer, theme, bounds, cursor)
    }
}

fn geometry(
    paint: &PickerPaint<'_>,
    state: &PickerState,
    renderer: &Renderer,
    _theme: &Theme,
    bounds: Rectangle,
    _cursor: Cursor,
) -> Vec<Geometry> {
    let mut frame = Frame::new(renderer, bounds.size());
    let mut text = state.text().borrow_mut();
    let text = text.get_or_insert_with(|| paint.skin().text_resources().into());
    let list = paint.base_commands(
        text,
        Rect {
            h: bounds.height,
            w: bounds.width,
            x: 0.0,
            y: 0.0,
        },
    );
    replay(
        &list,
        &mut IcedBackend::new(&mut frame, paint.skin().text_resources()),
    );
    vec![frame.into_geometry()]
}

pub(super) fn targets<'a>(
    path: &'a str,
    anchor: Rectangle,
    cursor: Cursor,
    open: bool,
    item_count: usize,
    item_height: f32,
) -> Vec<Target<'a>> {
    let mut targets = vec![Target::new(path, iced_interact::hit(anchor, cursor))];
    if !open {
        return targets;
    }
    for region in picker_hits(anchor.into(), item_height, item_count) {
        let bounds = region.area();
        targets.push(Target::item(
            path,
            iced_interact::hit(
                Rectangle {
                    x: bounds.x,
                    y: bounds.y,
                    width: bounds.w,
                    height: bounds.h,
                },
                cursor,
            ),
            *region.action(),
        ));
    }
    targets
}

#[cfg(test)]
mod tests {
    use iced::{
        Point,
        keyboard::{
            self, Location, Modifiers,
            key::{Code, Named, Physical},
        },
        mouse::Button,
        widget::canvas::Program as _,
    };
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        builtin,
        engine::PickerSnapshot,
        render::{ControlAction, InputOwner},
    };

    fn key_event(key: Named, code: Code) -> Event {
        Event::Keyboard(keyboard::Event::KeyPressed {
            key: keyboard::Key::Named(key),
            modified_key: keyboard::Key::Named(key),
            physical_key: Physical::Code(code),
            location: Location::Standard,
            modifiers: Modifiers::empty(),
            text: None,
            repeat: false,
        })
    }

    #[kithara::test]
    fn leaf_program_opens_navigates_and_selects_through_its_local_engine() {
        let skin = builtin::skin();
        let paint = Rc::new(PickerPaint::new(vec!["ZVUK", "LOCAL"], Some(0), skin));
        let program = InputProgram::new("library/context", paint);
        let mut state = PickerState::new("library/context", 2, Some(0), InputOwner::Leaf);
        let bounds = Rectangle {
            x: 10.0,
            y: 20.0,
            width: 72.0,
            height: skin.tree.scope_item_height,
        };
        let cursor = Cursor::Available(Point::new(40.0, 30.0));

        let opened = program
            .update(
                &mut state,
                &Event::Mouse(iced::mouse::Event::ButtonPressed(Button::Left)),
                bounds,
                cursor,
            )
            .unwrap_or_else(|| panic!("the leaf picker press must be answered"));
        assert!(opened.into_inner().0.is_none());
        assert!(state.snapshot().open);

        let navigated = program
            .update(
                &mut state,
                &key_event(Named::ArrowDown, Code::ArrowDown),
                bounds,
                Cursor::Unavailable,
            )
            .unwrap_or_else(|| panic!("the leaf picker arrow must be answered"));
        assert!(navigated.into_inner().0.is_none());
        assert_eq!(state.snapshot().highlighted, Some(1));

        let selected = program
            .update(
                &mut state,
                &key_event(Named::Enter, Code::Enter),
                bounds,
                Cursor::Unavailable,
            )
            .unwrap_or_else(|| panic!("the leaf picker Enter must be answered"));
        assert_eq!(
            selected.into_inner().0,
            Some(UiEvent::Control {
                path: "library/context".to_owned(),
                action: ControlAction::SelectIndex(1),
            })
        );
        assert!(!state.snapshot().open);
    }

    #[kithara::test]
    fn engine_owned_picker_state_has_no_local_engine() {
        let mut state = PickerState::new("library/context", 2, Some(0), InputOwner::Engine);

        assert!(state.engine_mut().is_none());
        assert_eq!(
            state.snapshot(),
            PickerSnapshot {
                open: false,
                highlighted: Some(0),
            }
        );
    }
}
