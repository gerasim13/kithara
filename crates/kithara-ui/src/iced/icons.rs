use iced::{
    Color, Element, Length,
    widget::{
        svg::{self, Handle as SvgHandle, Svg},
        text,
    },
};

use crate::{
    hosts::icons::{IconSource, source},
    module::IconName,
};

impl IconName {
    /// Renders this icon with the given size and color.
    #[must_use]
    pub fn view<'a, M: 'a>(self, size: f32, color: Color) -> Element<'a, M> {
        match source(self) {
            IconSource::Lucide(icon) => text(char::from(icon).to_string())
                .font(crate::iced::fonts::LUCIDE)
                .size(size)
                .color(color)
                .into(),
            IconSource::Svg(art) => Svg::new(SvgHandle::from_memory(art.document.as_bytes()))
                .width(Length::Fixed(size))
                .height(Length::Fixed(size))
                .style(move |_theme, _status| svg::Style { color: Some(color) })
                .into(),
        }
    }
}
