use crate::{
    draw::{DrawListBuilder, Pt, Rect, Rgba, Transform},
    module::ChipStyle,
    render::Skin,
    skin::{ColorRole, FontFamily, FontSkin, FrameSkin, TextRoleSkin},
    text::TextContext,
};

pub(crate) struct Chip {
    border: Rgba,
    fill: Rgba,
    frame: FrameSkin,
    padding: Pt,
    role: TextRoleSkin,
    text_color: Rgba,
}

impl Chip {
    pub(crate) fn new(style: ChipStyle, active: bool, skin: &Skin) -> Self {
        let frame = if active {
            skin.chip.active_frame
        } else {
            skin.chip.inactive_frame
        };
        let font: FontSkin = match style {
            ChipStyle::Deck => skin.chip.deck_text,
            ChipStyle::Routing => skin.chip.routing_text,
        };
        let text_color = if active {
            skin.palette.bg_deep
        } else {
            skin.palette.text_dim
        };
        Self {
            border: skin.rgba(frame.border),
            fill: if active {
                skin.palette.accent.into()
            } else {
                Rgba {
                    a: 0.0,
                    b: 0.0,
                    g: 0.0,
                    r: 0.0,
                }
            },
            frame,
            padding: Pt {
                x: skin.chip.padding_x,
                y: skin.chip.padding_y,
            },
            role: TextRoleSkin {
                color: ColorRole::Text,
                font: FontFamily::Mono,
                size: font.size,
                spacing: 0.0,
                weight: font.weight,
            },
            text_color: text_color.into(),
        }
    }

    pub(crate) fn paint(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        label: &str,
        bounds: Rect,
    ) {
        list.fill_rounded_rect(bounds, self.frame.radius, self.fill);
        self.paint_frame(list, bounds);
        let run = text.shape(label, self.role, None);
        list.text(
            &run,
            label,
            Transform::translate(Pt {
                x: bounds.x + self.padding.x,
                y: bounds.y + self.padding.y,
            }),
            self.text_color,
        );
    }

    fn paint_frame(&self, list: &mut DrawListBuilder, bounds: Rect) {
        if self.frame.border_width <= 0.0 {
            return;
        }
        let inset = self.frame.border_width / 2.0;
        list.stroke_rounded_rect(
            Rect {
                h: (bounds.h - self.frame.border_width).max(0.0),
                w: (bounds.w - self.frame.border_width).max(0.0),
                x: bounds.x + inset,
                y: bounds.y + inset,
            },
            self.frame.radius,
            self.border,
            self.frame.border_width,
        );
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        builtin,
        draw::{DrawCmd, DrawListBuilder, Geom, Rect},
        text::TextContext,
    };

    #[kithara::test]
    fn chip_paints_its_state_frame_and_shaped_label_in_order() {
        let skin = builtin::skin();
        let bounds = Rect {
            h: 16.0,
            w: 28.0,
            x: 3.0,
            y: 5.0,
        };
        let draw = |label, style, active| {
            let mut text = TextContext::from(skin.text_resources());
            let mut builder = DrawListBuilder::default();
            Chip::new(style, active, skin).paint(&mut builder, &mut text, label, bounds);
            builder.finish()
        };

        let active = draw("A", ChipStyle::Deck, true);
        let [fill, label] = active.commands() else {
            panic!("an active chip must draw its fill followed by its label");
        };
        assert!(matches!(
            fill,
            DrawCmd::Fill {
                geom: Geom::Rect(rect),
                color,
            } if *rect == bounds && *color == skin.palette.accent.into()
        ));
        assert!(matches!(
            label,
            DrawCmd::Text {
                run,
                content,
                color,
                transform,
                ..
            } if content == "A"
                && run.size() == skin.chip.deck_text.size
                && transform.dx == 11.0
                && transform.dy == 8.0
                && *color == skin.palette.bg_deep.into()
        ));

        let inactive = draw("FX1", ChipStyle::Routing, false);
        let [fill, frame, label] = inactive.commands() else {
            panic!("an inactive chip must draw its clear fill, frame, then label");
        };
        assert!(matches!(
            fill,
            DrawCmd::Fill {
                geom: Geom::Rect(rect),
                color,
            } if *rect == bounds && color.a == 0.0
        ));
        assert!(matches!(
            frame,
            DrawCmd::Stroke {
                geom: Geom::Rect(Rect {
                    h: 15.0,
                    w: 27.0,
                    x: 3.5,
                    y: 5.5,
                }),
                color,
                width: 1.0,
            } if *color == skin.rgba(skin.chip.inactive_frame.border)
        ));
        assert!(matches!(
            label,
            DrawCmd::Text {
                run,
                content,
                color,
                transform,
                ..
            } if content == "FX1"
                && run.size() == skin.chip.routing_text.size
                && transform.dx == 11.0
                && transform.dy == 8.0
                && *color == skin.palette.text_dim.into()
        ));
    }
}
