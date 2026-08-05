use iced::{
    Element,
    widget::{Text, text},
};

use crate::{
    backends::font,
    render::{IcedSkin, Skin, UiEvent},
    skin::TextRoleSkin,
};

/// Creates text with advanced shaping enabled.
pub fn shaped_text<'a, T: text::IntoFragment<'a>>(content: T) -> Text<'a> {
    Text::new(content).shaping(text::Shaping::Advanced)
}

pub(crate) fn styled_text(
    content: String,
    role: TextRoleSkin,
    skin: &Skin,
) -> Element<'static, UiEvent> {
    let font = font(role.font, role.weight);
    let color = skin.color(role.color);
    shaped_text(content)
        .font(font)
        .size(role.size)
        .color(color)
        .into()
}
