use iced::{Color, Element, Length, widget::container};

use crate::{
    render::{Icon, UiEvent},
    widgets::Widget,
};

#[derive(bon::Builder)]
pub(crate) struct Glyph {
    color: Color,
    icon: Icon,
    size: f32,
}

impl<'a> Widget<'a> for Glyph {
    fn view(self) -> Element<'a, UiEvent> {
        container(self.icon.view(self.size, self.color))
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }
}
