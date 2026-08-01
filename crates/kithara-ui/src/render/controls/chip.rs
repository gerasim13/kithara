use std::cell::RefCell;

use iced::{
    Element, Event, Length, Rectangle, Renderer, Theme,
    mouse::{Cursor, Interaction},
    widget::{
        Space,
        canvas::{self, Action, Canvas, Frame, Geometry},
    },
};

use crate::{
    atoms::chip::Chip,
    backends::IcedBackend,
    draw::{DrawListBuilder, Rect, replay},
    interact::{CursorShape, Hover, iced as iced_interact, recognizers::click},
    module::ChipStyle,
    render::{InputOwner, ReadValue, Skin, UiEvent, activate},
    text::{TextContext, TextResources},
};

pub(crate) fn chip<'a>(
    path: &'a str,
    label: &'a str,
    style: ChipStyle,
    value: Option<&ReadValue<'_>>,
    skin: &'a Skin,
    owner: InputOwner,
) -> Element<'a, UiEvent> {
    let Some(ReadValue::Bool(active)) = value else {
        return Space::new().into();
    };
    let paint = ChipPaint::new(label, style, *active, skin);
    match owner {
        InputOwner::Leaf => ChipProgram::new(path, paint).view(),
        InputOwner::Engine => paint.view(),
    }
}

struct ChipProgram<'data, 'skin> {
    paint: ChipPaint<'data, 'skin>,
    path: String,
}

impl<'data, 'skin> ChipProgram<'data, 'skin> {
    fn new(path: &str, paint: ChipPaint<'data, 'skin>) -> Self {
        Self {
            paint,
            path: path.to_owned(),
        }
    }

    fn view(self) -> Element<'skin, UiEvent>
    where
        'data: 'skin,
    {
        Canvas::new(self)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}

impl canvas::Program<UiEvent> for ChipProgram<'_, '_> {
    type State = ChipPaintState;

    fn draw(
        &self,
        state: &ChipPaintState,
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry> {
        self.paint.draw_with(state, renderer, theme, bounds)
    }

    fn mouse_interaction(
        &self,
        _state: &ChipPaintState,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Interaction {
        Hover::new(CursorShape::Pointer)
            .cursor(false, &iced_interact::hit(bounds, cursor))
            .into()
    }

    fn update(
        &self,
        _state: &mut ChipPaintState,
        event: &Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Option<Action<UiEvent>> {
        let input = iced_interact::input(event)?;
        let hit = iced_interact::hit(bounds, cursor);
        activate(&self.path, click::on_input(input, &hit))
    }
}

struct ChipPaint<'data, 'skin> {
    chip: Chip<'data, 'skin>,
    text_resources: &'skin TextResources,
}

impl<'data, 'skin> ChipPaint<'data, 'skin> {
    fn new(label: &'data str, style: ChipStyle, active: bool, skin: &'skin Skin) -> Self {
        Self {
            chip: Chip::new(label, style, active, skin),
            text_resources: skin.text_resources(),
        }
    }

    fn view(self) -> Element<'skin, UiEvent>
    where
        'data: 'skin,
    {
        Canvas::new(self)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn draw_with(
        &self,
        state: &ChipPaintState,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let mut text = state.text.borrow_mut();
        let text = text.get_or_insert_with(|| self.text_resources.into());
        let mut builder = DrawListBuilder::default();
        self.chip.paint(
            &mut builder,
            text,
            Rect {
                h: bounds.height,
                w: bounds.width,
                x: 0.0,
                y: 0.0,
            },
        );
        replay(
            &builder.finish(),
            &mut IcedBackend::new(&mut frame, self.text_resources),
        );
        vec![frame.into_geometry()]
    }
}

impl canvas::Program<UiEvent> for ChipPaint<'_, '_> {
    type State = ChipPaintState;

    fn draw(
        &self,
        state: &ChipPaintState,
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry> {
        self.draw_with(state, renderer, theme, bounds)
    }
}

#[derive(Default)]
struct ChipPaintState {
    text: RefCell<Option<TextContext>>,
}

#[cfg(test)]
mod tests {
    use iced::{Point, event, mouse, window::RedrawRequest};
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{builtin, render::ControlAction};

    #[kithara::test]
    fn the_leaf_chip_uses_the_shared_activation_gesture() {
        let paint = ChipPaint::new("A", ChipStyle::Deck, true, builtin::skin());
        let program = ChipProgram::new("atoms/chips/active", paint);
        let bounds = Rectangle {
            height: 16.0,
            width: 20.0,
            x: 0.0,
            y: 0.0,
        };
        let cursor = Cursor::Available(Point::new(10.0, 8.0));
        let press = Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));
        let mut state = ChipPaintState::default();

        let action = canvas::Program::update(&program, &mut state, &press, bounds, cursor)
            .unwrap_or_else(|| panic!("a chip press inside its bounds must publish"));

        assert_eq!(
            action.into_inner(),
            (
                Some(UiEvent::Control {
                    path: "atoms/chips/active".to_owned(),
                    action: ControlAction::Activate,
                }),
                RedrawRequest::Wait,
                event::Status::Captured,
            )
        );
    }
}
