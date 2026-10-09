use num_traits::{ToPrimitive, cast::AsPrimitive};

use crate::{
    atoms::design::picker::Picker,
    draw::{DrawList, DrawListBuilder, Rect, Rgba},
    hosts::layer::{HostLayer, LayerHit},
    interact::CursorShape,
    render::{ReadValue, Skin},
    shaping::TextContext,
};

/// The open menu, drawn from the skin alone.
///
/// Both hosts raise this layer for themselves — the immediate one as an iced
/// overlay, the retained one as a Masonry layer above the tree — so the menu
/// keeps its own painter rather than one copy per host. Everything it needs is
/// taken from the skin here, because a layer outlives the borrow the document
/// walk had.
pub(crate) struct PickerMenu {
    face: Picker,
    background: Rgba,
    border: Rgba,
    selected_background: Rgba,
    selected_text: Rgba,
    text: Rgba,
    border_width: f32,
    item_height: f32,
    radius: f32,
}

impl PickerMenu {
    pub(crate) fn new(skin: &Skin) -> Self {
        let metrics = &skin.tree;
        Self {
            background: skin.rgba(metrics.scope_menu_background),
            border: skin.rgba(metrics.scope_menu_frame.border),
            border_width: metrics.scope_menu_frame.border_width,
            face: Picker::new(skin),
            item_height: metrics.scope_item_height,
            radius: metrics.scope_menu_frame.radius,
            selected_background: skin.rgba(metrics.scope_selected_background),
            selected_text: skin.rgba(metrics.scope_selected_text),
            text: skin.rgba(metrics.scope_menu_text),
        }
    }

    fn commands(
        &self,
        text: &mut TextContext,
        width: f32,
        items: &[&str],
        highlighted: Option<usize>,
    ) -> DrawList {
        let bounds = Rect {
            h: self.item_height * AsPrimitive::<f32>::as_(items.len()),
            w: width,
            x: 0.0,
            y: 0.0,
        };
        let mut list = DrawListBuilder::default();
        list.fill_rounded_rect(bounds, self.radius, self.background);
        for (index, label) in items.iter().enumerate() {
            let item = Rect {
                h: self.item_height,
                w: bounds.w,
                x: 0.0,
                y: AsPrimitive::<f32>::as_(index) * self.item_height,
            };
            let active = highlighted == Some(index);
            if active {
                list.fill_rect(item, self.selected_background);
            }
            self.face.label(
                &mut list,
                text,
                label,
                item,
                if active {
                    self.selected_text
                } else {
                    self.text
                },
            );
        }
        list.stroke_rounded_rect(bounds, self.radius, self.border, self.border_width);
        list.finish()
    }

    /// The menu hanging off `anchor`: its own unclipped frame, and one hit per
    /// option.
    pub(crate) fn layer<'a>(
        &self,
        text: &mut TextContext,
        anchor: Rect,
        items: impl IntoIterator<Item = &'a str>,
        highlighted: Option<usize>,
    ) -> HostLayer<usize> {
        let items: Vec<&str> = items.into_iter().collect();
        let bounds = Rect {
            h: self.item_height * AsPrimitive::<f32>::as_(items.len()),
            w: anchor.w,
            x: anchor.x,
            y: anchor.y + anchor.h,
        };
        HostLayer::new(
            bounds,
            self.commands(text, bounds.w, &items, highlighted),
            picker_hits(anchor, self.item_height, items.len()),
        )
    }
}

pub(crate) fn picker_selected_index(
    value: Option<&ReadValue<'_>>,
    item_count: usize,
) -> Option<usize> {
    let last = item_count.checked_sub(1)?;
    let ReadValue::Scalar(value) = value? else {
        return None;
    };
    value.round().to_usize().map(|index| index.min(last))
}

fn picker_option_bounds(anchor: Rect, item_height: f32, index: usize) -> Rect {
    Rect {
        h: item_height,
        w: anchor.w,
        x: anchor.x,
        y: anchor.y + anchor.h + AsPrimitive::<f32>::as_(index) * item_height,
    }
}

pub(crate) fn picker_hits(
    anchor: Rect,
    item_height: f32,
    item_count: usize,
) -> Vec<LayerHit<usize>> {
    (0..item_count)
        .map(|index| {
            LayerHit::new(
                picker_option_bounds(anchor, item_height, index),
                CursorShape::Pointer,
                index,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test]
    fn option_hit_rectangles_start_below_the_anchor() {
        let anchor = Rect {
            h: 22.0,
            w: 72.0,
            x: 14.0,
            y: 18.0,
        };
        assert_eq!(
            picker_option_bounds(anchor, 20.0, 1),
            Rect {
                h: 20.0,
                w: 72.0,
                x: 14.0,
                y: 60.0,
            }
        );
    }
}
