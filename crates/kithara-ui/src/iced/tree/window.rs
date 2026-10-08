use iced::Element;

use super::{drag::drag_root, node::IcedHost};
use crate::{
    compile::{CompiledNode, CompiledUi},
    hosts::window::{TitleBar, WindowControls},
    iced::tree::Widget,
    ids::InternId,
    module::WindowControlsStyle,
    render::{
        Published, Reads, Skin,
        custom::CustomKinds,
        document::{self, Clock, Ctx},
    },
    view::ViewState,
};

/// Draws one frame of the document.
///
/// `clock` is this host's own reading of time, so a caller that drives it
/// reproduces a frame exactly rather than waiting for a wall clock.
///
/// `view` is the state this screen keeps for itself, which the host owns for
/// as long as it shows the screen.
///
/// `kinds` are the extensions the application registered. Nothing registered is
/// the ordinary case; a document that names one was refused while it compiled
/// unless the same set was declared to `UiConfig`.
pub fn render<'a>(
    node: &CompiledNode,
    ui: &'a CompiledUi,
    reads: &dyn Reads,
    view: &ViewState,
    skin: &'a Skin,
    clock: Clock,
    kinds: Option<&'a CustomKinds>,
) -> Element<'a, Published> {
    let ctx = Ctx::new(ui, reads, view, skin.document(), clock);
    let ctx = kinds.map_or(ctx, |kinds| ctx.with_kinds(kinds));
    drag_root(document::render(node, ctx, IcedHost::new(ctx, skin)), skin)
}

pub(super) fn titlebar<'a>(
    label: InternId,
    ui: &'a CompiledUi,
    skin: &Skin,
) -> Element<'a, Published> {
    TitleBar::builder()
        .label(ui.resolve(label))
        .skin(skin)
        .build()
        .view()
}

pub(super) fn window_controls(
    style: WindowControlsStyle,
    skin: &Skin,
) -> Element<'static, Published> {
    WindowControls::builder()
        .style(style)
        .skin(skin)
        .build()
        .view()
}
