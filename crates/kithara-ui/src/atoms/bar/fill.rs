use crate::draw::{DrawListBuilder, Rect, Rgba};

#[derive(Clone, PartialEq, kithara_derive::ControlPainter)]
#[control_painter(data = (), draw = self.paint(list, bounds))]
pub(crate) struct Fill {
    color: Rgba,
}

impl Fill {
    pub(crate) const fn new(color: Rgba) -> Self {
        Self { color }
    }

    pub(crate) fn paint(&self, list: &mut DrawListBuilder, bounds: Rect) {
        list.fill_rect(bounds, self.color);
    }
}
