use crate::{
    draw::{DrawListBuilder, Rect, Rgba},
    render::Skin,
};

/// Empty room that pushes its neighbours apart. It draws nothing but the
/// panel behind it, which is what keeps the bar continuous across the gap.
#[derive(Clone, PartialEq, kithara_derive::ControlPainter)]
#[control_painter(
    data = (),
    draw = self.paint(list, bounds)
)]
#[derive(kithara_derive::Retained)]
pub(crate) struct Spacer {
    panel: Rgba,
}

impl Spacer {
    pub(crate) fn new(skin: &Skin) -> Self {
        Self {
            panel: skin.rgba(skin.global_bar.panel_fill),
        }
    }

    pub(crate) fn paint(&self, list: &mut DrawListBuilder, bounds: Rect) {
        list.fill_rect(bounds, self.panel);
    }
}
