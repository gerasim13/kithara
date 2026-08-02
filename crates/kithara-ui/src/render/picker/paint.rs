use num_traits::{ToPrimitive, cast::AsPrimitive};

use crate::{
    draw::{DrawList, DrawListBuilder, Pt, Rect, Transform},
    render::{ReadValue, Skin},
    skin::{FontFamily, TextRoleSkin},
    text::TextContext,
};

pub(super) struct PickerPaint<'a> {
    items: Vec<&'a str>,
    selected: Option<usize>,
    skin: &'a Skin,
    width: f32,
}

impl<'a> PickerPaint<'a> {
    pub(super) fn new(items: Vec<&'a str>, selected: Option<usize>, skin: &'a Skin) -> Self {
        let mut text = TextContext::from(skin.text_resources());
        let label_width = items
            .iter()
            .map(|item| text.shape(item, text_role(skin), None).width())
            .fold(0.0_f32, f32::max);
        let width = label_width + skin.tree.scope_padding_x * 3.0 + skin.tree.scope_chevron_size;
        Self {
            items,
            selected,
            skin,
            width,
        }
    }

    pub(super) const fn item_count(&self) -> usize {
        self.items.len()
    }

    pub(super) const fn item_height(&self) -> f32 {
        self.skin.tree.scope_item_height
    }

    pub(super) const fn selected(&self) -> Option<usize> {
        self.selected
    }

    pub(super) const fn skin(&self) -> &Skin {
        self.skin
    }

    pub(super) const fn width(&self) -> f32 {
        self.width
    }

    pub(super) fn base_commands(&self, text: &mut TextContext, bounds: Rect) -> DrawList {
        let mut list = DrawListBuilder::default();
        list.fill_rounded_rect(
            bounds,
            self.skin.tree.scope_frame.radius,
            self.skin.rgba(self.skin.tree.scope_background),
        );
        list.stroke_rounded_rect(
            bounds,
            self.skin.tree.scope_frame.radius,
            self.skin.rgba(self.skin.tree.scope_frame.border),
            self.skin.tree.scope_frame.border_width,
        );
        if let Some(label) = self.selected.and_then(|index| self.items.get(index)) {
            paint_text(
                &mut list,
                text,
                label,
                bounds,
                self.skin.tree.scope_padding_x,
                self.skin.rgba(self.skin.tree.scope_text_color),
                self.skin,
            );
        }
        let size = self.skin.tree.scope_chevron_size;
        let center = Pt {
            x: bounds.x + bounds.w - self.skin.tree.scope_padding_x - size / 2.0,
            y: bounds.y + bounds.h / 2.0,
        };
        let half = size / 2.0;
        let color = self.skin.rgba(self.skin.tree.scope_chevron_color);
        let width = self.skin.tree.scope_frame.border_width.max(1.0);
        list.stroke_line(
            Pt {
                x: center.x - half,
                y: center.y - half / 2.0,
            },
            Pt {
                x: center.x,
                y: center.y + half / 2.0,
            },
            color,
            width,
        );
        list.stroke_line(
            Pt {
                x: center.x,
                y: center.y + half / 2.0,
            },
            Pt {
                x: center.x + half,
                y: center.y - half / 2.0,
            },
            color,
            width,
        );
        list.finish()
    }

    pub(super) fn popup_commands(
        &self,
        text: &mut TextContext,
        width: f32,
        highlighted: Option<usize>,
    ) -> DrawList {
        let bounds = Rect {
            h: self.item_height() * AsPrimitive::<f32>::as_(self.items.len()),
            w: width,
            x: 0.0,
            y: 0.0,
        };
        let mut list = DrawListBuilder::default();
        list.fill_rounded_rect(
            bounds,
            self.skin.tree.scope_menu_frame.radius,
            self.skin.rgba(self.skin.tree.scope_menu_background),
        );
        for (index, label) in self.items.iter().enumerate() {
            let item = Rect {
                h: self.item_height(),
                w: bounds.w,
                x: 0.0,
                y: AsPrimitive::<f32>::as_(index) * self.item_height(),
            };
            let active = highlighted == Some(index);
            if active {
                list.fill_rect(
                    item,
                    self.skin.rgba(self.skin.tree.scope_selected_background),
                );
            }
            paint_text(
                &mut list,
                text,
                label,
                item,
                self.skin.tree.scope_padding_x,
                self.skin.rgba(if active {
                    self.skin.tree.scope_selected_text
                } else {
                    self.skin.tree.scope_menu_text
                }),
                self.skin,
            );
        }
        list.stroke_rounded_rect(
            bounds,
            self.skin.tree.scope_menu_frame.radius,
            self.skin.rgba(self.skin.tree.scope_menu_frame.border),
            self.skin.tree.scope_menu_frame.border_width,
        );
        list.finish()
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

pub(crate) fn picker_option_bounds(anchor: Rect, item_height: f32, index: usize) -> Rect {
    Rect {
        h: item_height,
        w: anchor.w,
        x: anchor.x,
        y: anchor.y + anchor.h + AsPrimitive::<f32>::as_(index) * item_height,
    }
}

fn text_role(skin: &Skin) -> TextRoleSkin {
    TextRoleSkin {
        color: skin.tree.scope_text_color,
        font: FontFamily::Mono,
        size: skin.tree.scope_text.size,
        spacing: 0.0,
        weight: skin.tree.scope_text.weight,
    }
}

fn paint_text(
    list: &mut DrawListBuilder,
    text: &mut TextContext,
    content: &str,
    bounds: Rect,
    padding_x: f32,
    color: crate::draw::Rgba,
    skin: &Skin,
) {
    let run = text.shape(
        content,
        text_role(skin),
        Some((bounds.w - padding_x * 2.0).max(0.0)),
    );
    list.text(
        &run,
        content,
        Transform::translate(Pt {
            x: bounds.x + padding_x,
            y: bounds.y + (bounds.h - run.height()) / 2.0,
        }),
        color,
    );
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{builtin, draw::DrawCmd};

    #[kithara::test]
    fn popup_commands_are_a_separate_unclipped_frame_list() {
        let skin = builtin::skin();
        let paint = PickerPaint::new(vec!["ZVUK", "LOCAL"], Some(0), skin);
        let mut text = TextContext::from(skin.text_resources());
        let bounds = Rect {
            h: skin.tree.scope_item_height,
            w: paint.width(),
            x: 0.0,
            y: 0.0,
        };
        let base = paint.base_commands(&mut text, bounds);
        let popup = paint.popup_commands(&mut text, bounds.w, Some(1));

        assert!(
            base.commands()
                .iter()
                .all(|command| !matches!(command, DrawCmd::Clip { .. })),
            "the anchor list must not contain a nested popup clip"
        );
        assert!(
            popup
                .commands()
                .iter()
                .all(|command| !matches!(command, DrawCmd::Clip { .. })),
            "the fresh overlay frame must receive an unclipped popup list"
        );
        assert!(base.commands().iter().all(|command| {
            !matches!(command, DrawCmd::Text { content, .. } if content == "LOCAL")
        }));
        assert!(popup.commands().iter().any(|command| {
            matches!(command, DrawCmd::Text { content, .. } if content == "LOCAL")
        }));
        assert!(matches!(
            popup.commands(),
            [
                DrawCmd::Fill { .. },
                DrawCmd::Text { .. },
                DrawCmd::Fill { .. },
                DrawCmd::Text { .. },
                DrawCmd::Stroke { .. },
            ]
        ));
    }

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
