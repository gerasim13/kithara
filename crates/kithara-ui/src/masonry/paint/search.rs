use crate::{
    atoms::{search::paint::paint_shaped, text_input::TextInputPaint},
    draw::{DrawListBuilder, Rect},
    engine::TextInputSnapshot,
    render::Skin,
    shaping::TextContext,
};

pub(crate) fn paint(
    list: &mut DrawListBuilder,
    text: &mut TextContext,
    bounds: Rect,
    query: &str,
    skin: &Skin,
    snapshot: &TextInputSnapshot,
) {
    let input = TextInputPaint::with_context(query.into(), skin, text);
    paint_shaped(list, text, bounds, &input, skin, snapshot);
}
