use crate::{
    atoms::icon::mark::Marked,
    draw::{DrawListBuilder, Rect, Rgba},
    render::Mark,
    shaping::TextContext,
};

/// One icon on its own, centred in the box it was given.
///
/// Unlike the icon inside a button or a rail item, this one is the whole
/// control: nothing sits beside it, so it has the middle of the box rather than
/// a left edge to start from.
#[derive(Clone, PartialEq)]
pub(crate) struct Glyph {
    active_color: Rgba,
    color: Rgba,
    size: f32,
}

/// What a glyph is handed each frame: the art for each of its states, and which
/// state it is in.
///
/// Both marks travel together because a document may name a different icon for
/// the active state, and reading authored art can fail — so the choice is made
/// once, where it can still answer "nothing to draw".
#[derive(Clone, PartialEq)]
pub(crate) struct GlyphData {
    pub(crate) mark: Mark,
    pub(crate) active_mark: Option<Mark>,
    pub(crate) active: bool,
}

impl Glyph {
    pub(crate) const fn new(color: Rgba, active_color: Rgba, size: f32) -> Self {
        Self {
            active_color,
            color,
            size,
        }
    }

    pub(crate) fn paint(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        data: &GlyphData,
        bounds: Rect,
    ) {
        let (mark, color) = if data.active {
            (data.active_mark.unwrap_or(data.mark), self.active_color)
        } else {
            (data.mark, self.color)
        };
        Marked::new(mark, self.size).centred(list, text, bounds, color);
    }
}
