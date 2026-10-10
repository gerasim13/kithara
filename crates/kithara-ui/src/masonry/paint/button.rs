use crate::{atoms::button::face::Width, hosts::solve::Length, module::ButtonStyle, render::Skin};

/// What a parent has to be told about a button's width before the button
/// exists.
///
/// A retained host settles a row's shares while it is still walking the
/// document, which is earlier than it holds a painter — so this reads the same
/// table the painter reads rather than restating it.
///
/// Only the retained host asks: the immediate one reads the box off the built
/// widget, which by then holds the painter.
pub(crate) fn declared_width(style: ButtonStyle, skin: &Skin) -> Length {
    Width::new(style, skin).length()
}
