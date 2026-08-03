use iced::Element;

use super::{node::render_compiled, read::resolve};
use crate::{
    compile::{CompiledNode, CompiledUi},
    ids::InternId,
    module::WindowControlsStyle,
    render::{ReadValue, Reads, Skin, UiEvent, window_layers},
    widgets::{
        Widget,
        window::{TitleBar, WindowControls},
    },
};

pub fn render<'a>(
    node: &CompiledNode,
    ui: &'a CompiledUi,
    reads: &dyn Reads,
    skin: &'a Skin,
) -> Element<'a, UiEvent> {
    let content = render_compiled(node, ui, reads, skin);
    if !ui.resize_edges && ui.dragged.is_none() {
        return content;
    }
    window_layers(content, dragged_label(ui, reads), ui.resize_edges, skin)
}

fn dragged_label(ui: &CompiledUi, reads: &dyn Reads) -> Option<String> {
    let binding = ui.dragged.as_ref()?;
    match resolve(reads, binding, ui)? {
        ReadValue::Text(label) if !label.is_empty() => Some(label.to_owned()),
        _ => None,
    }
}

pub(super) fn titlebar<'a>(
    label: InternId,
    ui: &'a CompiledUi,
    skin: &Skin,
) -> Element<'a, UiEvent> {
    TitleBar::builder()
        .label(ui.resolve(label))
        .skin(skin)
        .build()
        .view()
}

pub(super) fn window_controls(
    style: WindowControlsStyle,
    skin: &Skin,
) -> Element<'static, UiEvent> {
    WindowControls::builder()
        .style(style)
        .skin(skin)
        .build()
        .view()
}
