use std::cell::RefCell;

use iced::{
    Element, Length, Rectangle, Renderer, Size, Theme, Vector,
    advanced::{
        Renderer as _, Widget as IcedWidget,
        graphics::geometry::Renderer as _,
        layout::{self, Layout},
        mouse, renderer,
        widget::{self, Tree},
    },
    widget::canvas::Frame,
};

use crate::{
    atoms::text::Text as TextAtom,
    backends::replay_ordered,
    draw::{DrawListBuilder, Rect},
    module::TextStyle,
    render::{ReadValue, Skin, UiEvent, Widget},
    skin::{ColorRole, TextRoleSkin},
    text::TextContext,
};

#[derive(bon::Builder)]
pub(crate) struct Text<'value, 'data, 'skin> {
    skin: &'skin Skin,
    active_color: Option<ColorRole>,
    color: Option<ColorRole>,
    label: Option<&'data str>,
    value: Option<&'value ReadValue<'data>>,
    style: TextStyle,
    active: bool,
}

impl<'a, 'value, 'data, 'skin> Widget<'a> for Text<'value, 'data, 'skin>
where
    'skin: 'a,
{
    fn view(self) -> Element<'a, UiEvent> {
        let value = match self.value {
            Some(ReadValue::Text(value)) => Some(*value),
            _ => self.label,
        };
        let Some(value) = value else {
            return iced::widget::Space::new().into();
        };
        let role = self
            .skin
            .text_role(self.style, self.color, self.active_color, self.active);
        let content = if self.style == TextStyle::MicroLabel {
            value.to_uppercase()
        } else {
            value.to_owned()
        };
        let padding_x = match self.style {
            TextStyle::VisFooter => self.skin.vis.footer_padding_x,
            TextStyle::VisMeta => self.skin.vis.index_padding_x,
            TextStyle::VisTitle => self.skin.vis.name_padding_x,
            _ => 0.0,
        };
        Painted {
            content,
            padding_x,
            role,
            skin: self.skin,
        }
        .into()
    }
}

struct Painted<'skin> {
    content: String,
    padding_x: f32,
    role: TextRoleSkin,
    skin: &'skin Skin,
}

#[derive(Default)]
struct State {
    text: RefCell<Option<TextContext>>,
}

impl IcedWidget<UiEvent, Theme, Renderer> for Painted<'_> {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Shrink, Length::Fill)
    }

    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<State>()
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(State::default())
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        _renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let state = tree.state.downcast_mut::<State>();
        let mut text = state.text.borrow_mut();
        let text = text.get_or_insert_with(|| self.skin.text_resources().into());
        let (width, height) =
            TextAtom::new(&self.content, self.role, self.padding_x, self.skin).measure(text);
        layout::Node::new(limits.resolve(Length::Shrink, Length::Fill, Size::new(width, height)))
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();

        if bounds.width < 1.0 || bounds.height < 1.0 {
            return;
        }

        let state = tree.state.downcast_ref::<State>();
        let mut text = state.text.borrow_mut();
        let text = text.get_or_insert_with(|| self.skin.text_resources().into());

        renderer.with_translation(Vector::new(bounds.x, bounds.y), |renderer| {
            let mut frame = Frame::new(renderer, bounds.size());
            let mut builder = DrawListBuilder::default();
            TextAtom::new(&self.content, self.role, self.padding_x, self.skin).paint(
                &mut builder,
                text,
                Rect {
                    h: bounds.height,
                    w: bounds.width,
                    x: 0.0,
                    y: 0.0,
                },
            );
            replay_ordered(&builder.finish(), &mut frame, self.skin.text_resources());
            renderer.draw_geometry(frame.into_geometry());
        });
    }
}

impl<'a> From<Painted<'a>> for Element<'a, UiEvent> {
    fn from(painted: Painted<'a>) -> Self {
        Self::new(painted)
    }
}
