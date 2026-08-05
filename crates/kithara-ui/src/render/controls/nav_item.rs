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
    atoms::nav_item::NavItem,
    backends::replay_ordered,
    draw::{DrawList, DrawListBuilder, Rect},
    interact::{CursorShape, Hover, iced as iced_interact, recognizers::click},
    render::{Icon, InputOwner, Mark, ReadValue, Skin, UiEvent, activate},
    text::{TextContext, TextResources},
};

pub(crate) fn nav_item<'a>(
    path: &'a str,
    label: &'a str,
    icon: Icon,
    value: Option<&ReadValue<'_>>,
    skin: &'a Skin,
    owner: InputOwner,
) -> Element<'a, UiEvent> {
    let (Some(ReadValue::Bool(active)), Some(mark)) = (value, icon.mark()) else {
        return Space::new().into();
    };
    let paint = NavItemPaint::new(label, mark, *active, skin);
    match owner {
        InputOwner::Leaf => NavItemProgram::new(path, paint).view(),
        InputOwner::Engine => paint.view(),
    }
}

struct NavItemProgram<'data, 'skin> {
    paint: NavItemPaint<'data, 'skin>,
    path: String,
}

impl<'data, 'skin> NavItemProgram<'data, 'skin> {
    fn new(path: &str, paint: NavItemPaint<'data, 'skin>) -> Self {
        Self {
            paint,
            path: path.to_owned(),
        }
    }

    fn view(self) -> Element<'skin, UiEvent>
    where
        'data: 'skin,
    {
        let height = self.paint.height();
        Canvas::new(self)
            .width(Length::Fill)
            .height(Length::Fixed(height))
            .into()
    }
}

impl canvas::Program<UiEvent> for NavItemProgram<'_, '_> {
    type State = NavItemPaintState;

    fn draw(
        &self,
        state: &NavItemPaintState,
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry> {
        self.paint.draw_with(state, renderer, theme, bounds)
    }

    fn mouse_interaction(
        &self,
        _state: &NavItemPaintState,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Interaction {
        Hover::new(CursorShape::Pointer)
            .cursor(false, &iced_interact::hit(bounds, cursor))
            .into()
    }

    fn update(
        &self,
        _state: &mut NavItemPaintState,
        event: &Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Option<Action<UiEvent>> {
        let input = iced_interact::input(event)?;
        let hit = iced_interact::hit(bounds, cursor);
        activate(&self.path, click::on_input(input, &hit))
    }
}

#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
struct NavItemPaint<'data, 'skin> {
    active: bool,
    #[field(get, vis = "")]
    height: f32,
    item: NavItem,
    label: &'data str,
    text_resources: &'skin TextResources,
}

impl<'data, 'skin> NavItemPaint<'data, 'skin> {
    fn new(label: &'data str, mark: Mark, active: bool, skin: &'skin Skin) -> Self {
        Self {
            active,
            height: skin.nav.item_height,
            item: NavItem::new(mark, skin),
            label,
            text_resources: skin.text_resources(),
        }
    }

    fn view(self) -> Element<'skin, UiEvent>
    where
        'data: 'skin,
    {
        let height = self.height();
        Canvas::new(self)
            .width(Length::Fill)
            .height(Length::Fixed(height))
            .into()
    }

    fn draw_with(
        &self,
        state: &NavItemPaintState,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let list = self.draw_list(
            state,
            Rect {
                h: bounds.height,
                w: bounds.width,
                x: 0.0,
                y: 0.0,
            },
        );
        replay_ordered(&list, &mut frame, self.text_resources);
        vec![frame.into_geometry()]
    }

    fn draw_list(&self, state: &NavItemPaintState, bounds: Rect) -> DrawList {
        let mut text = state.text.borrow_mut();
        let text = text.get_or_insert_with(|| self.text_resources.into());
        let mut builder = DrawListBuilder::default();
        self.item
            .paint(&mut builder, text, self.label, self.active, bounds);
        builder.finish()
    }
}

impl canvas::Program<UiEvent> for NavItemPaint<'_, '_> {
    type State = NavItemPaintState;

    fn draw(
        &self,
        state: &NavItemPaintState,
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry> {
        self.draw_with(state, renderer, theme, bounds)
    }
}

#[derive(Default)]
struct NavItemPaintState {
    text: RefCell<Option<TextContext>>,
}

#[cfg(test)]
mod tests {
    use iced::{Point, event, mouse, window::RedrawRequest};
    use kithara_test_utils::kithara;

    use super::*;
    #[cfg(feature = "masonry-host")]
    use crate::{
        atoms::painter::Labelled,
        render::masonry::{MasonryControl, Painted},
    };
    use crate::{builtin, render::ControlAction};

    /// Both kinds of icon, on both hosts. The authored one is here because it
    /// used to leave the draw list for an iced widget, which left the retained
    /// host with nothing to paint at all.
    #[cfg(feature = "masonry-host")]
    #[kithara::test]
    fn iced_and_masonry_record_the_same_nav_item() {
        let skin = builtin::skin();
        let bounds = Rect {
            h: 30.0,
            w: 198.0,
            x: 0.0,
            y: 0.0,
        };
        for icon in [Icon::Play, Icon::Zvuk] {
            let mark = icon
                .mark()
                .unwrap_or_else(|| panic!("{icon:?} must have a mark"));
            for active in [false, true] {
                let iced = NavItemPaint::new("BUTTONS", mark, active, skin)
                    .draw_list(&NavItemPaintState::default(), bounds);
                let mut masonry = Painted::new(
                    NavItem::new(mark, skin),
                    Labelled {
                        active,
                        label: "BUTTONS".to_owned(),
                    },
                    skin,
                );

                assert_eq!(iced, MasonryControl::draw_list(&mut masonry, bounds));
                assert!(
                    iced.commands().len() >= 4,
                    "{icon:?} must draw its icon alongside the rest"
                );
            }
        }
    }

    #[kithara::test]
    fn the_leaf_nav_item_uses_the_shared_activation_gesture() {
        let mark = Icon::Play
            .mark()
            .unwrap_or_else(|| panic!("the play icon must have a mark"));
        let paint = NavItemPaint::new("BUTTONS", mark, false, builtin::skin());
        let program = NavItemProgram::new("gallery/buttons/item", paint);
        let bounds = Rectangle {
            height: 30.0,
            width: 198.0,
            x: 0.0,
            y: 0.0,
        };
        let cursor = Cursor::Available(Point::new(99.0, 15.0));
        let press = Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));
        let mut state = NavItemPaintState::default();

        let action = canvas::Program::update(&program, &mut state, &press, bounds, cursor)
            .unwrap_or_else(|| panic!("a nav item press inside its bounds must publish"));

        assert_eq!(
            action.into_inner(),
            (
                Some(UiEvent::Control {
                    path: "gallery/buttons/item".to_owned(),
                    action: ControlAction::Activate,
                }),
                RedrawRequest::Wait,
                event::Status::Captured,
            )
        );
    }

    /// Both kinds of icon reach the draw list, so a nav item never has to be
    /// handed to a toolkit widget to be seen.
    #[kithara::test]
    fn a_nav_item_draws_a_glyph_and_an_outline_alike() {
        assert!(matches!(Icon::Play.mark(), Some(Mark::Glyph(_))));
        assert!(matches!(Icon::Zvuk.mark(), Some(Mark::Outline(_))));
    }
}
