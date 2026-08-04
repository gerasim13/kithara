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
    backends::IcedBackend,
    draw::{DrawList, DrawListBuilder, Rect, replay},
    interact::{CursorShape, Hover, iced as iced_interact, recognizers::click},
    render::{Icon, InputOwner, ReadValue, Skin, UiEvent, activate},
    text::{TextContext, TextResources},
    widgets::{Widget, nav::NavItem as IcedNavItem},
};

pub(crate) fn nav_item<'a>(
    path: &'a str,
    label: &'a str,
    icon: Icon,
    value: Option<&ReadValue<'_>>,
    skin: &'a Skin,
    owner: InputOwner,
) -> Element<'a, UiEvent> {
    let Some(ReadValue::Bool(active)) = value else {
        return Space::new().into();
    };
    let Some(glyph) = icon.lucide_glyph() else {
        return IcedNavItem::builder()
            .path(path)
            .label(label)
            .icon(icon)
            .maybe_value(value)
            .skin(skin)
            .build()
            .view();
    };
    let paint = NavItemPaint::new(label, glyph, *active, skin);
    match owner {
        InputOwner::Leaf => NavItemProgram::new(path, paint).view(),
        InputOwner::Engine => paint.view(),
    }
}

pub(crate) fn nav_item_supports_engine_input(icon: Icon) -> bool {
    icon.lucide_glyph().is_some()
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
    #[field(get, vis = "")]
    height: f32,
    item: NavItem,
    label: &'data str,
    text_resources: &'skin TextResources,
}

impl<'data, 'skin> NavItemPaint<'data, 'skin> {
    fn new(label: &'data str, glyph: char, active: bool, skin: &'skin Skin) -> Self {
        Self {
            height: skin.nav.item_height,
            item: NavItem::new(glyph, active, skin),
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
        replay(
            &list,
            &mut IcedBackend::new(&mut frame, self.text_resources),
        );
        vec![frame.into_geometry()]
    }

    fn draw_list(&self, state: &NavItemPaintState, bounds: Rect) -> DrawList {
        let mut text = state.text.borrow_mut();
        let text = text.get_or_insert_with(|| self.text_resources.into());
        let mut builder = DrawListBuilder::default();
        self.item.paint(&mut builder, text, self.label, bounds);
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
    use crate::render::masonry::{Click, MasonryControl};
    use crate::{builtin, render::ControlAction};

    #[cfg(feature = "masonry-host")]
    #[kithara::test]
    fn iced_and_masonry_record_the_same_nav_item() {
        let skin = builtin::skin();
        let glyph = Icon::Play
            .lucide_glyph()
            .unwrap_or_else(|| panic!("the play icon must be a Lucide glyph"));
        let bounds = Rect {
            h: 30.0,
            w: 198.0,
            x: 0.0,
            y: 0.0,
        };
        for active in [false, true] {
            let iced = NavItemPaint::new("BUTTONS", glyph, active, skin)
                .draw_list(&NavItemPaintState::default(), bounds);
            let mut masonry = Click::new(
                NavItem::new(glyph, active, skin),
                "BUTTONS".to_owned(),
                skin,
            );

            assert_eq!(iced, MasonryControl::draw_list(&mut masonry, bounds));
        }
    }

    #[kithara::test]
    fn the_leaf_nav_item_uses_the_shared_activation_gesture() {
        let glyph = Icon::Play
            .lucide_glyph()
            .unwrap_or_else(|| panic!("the play icon must be a Lucide glyph"));
        let paint = NavItemPaint::new("BUTTONS", glyph, false, builtin::skin());
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

    #[kithara::test]
    fn only_lucide_nav_items_offer_engine_input() {
        assert!(nav_item_supports_engine_input(Icon::Play));
        assert!(!nav_item_supports_engine_input(Icon::PlayReverse));
    }
}
