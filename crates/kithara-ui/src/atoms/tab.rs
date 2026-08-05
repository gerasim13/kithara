use crate::{
    draw::{DrawListBuilder, Pt, Rect, Rgba, Transform},
    render::Skin,
    skin::{ColorRole, FontFamily, FontWeight, TabLargeSkin, TextRoleSkin},
    text::{GlyphRun, TextContext},
};

pub(crate) struct TabLarge {
    active_color: Rgba,
    idle_color: Rgba,
    metrics: TabLargeSkin,
    role: TextRoleSkin,
    underline: Rgba,
}

impl TabLarge {
    pub(crate) fn new(skin: &Skin) -> Self {
        Self {
            active_color: skin.palette.text,
            idle_color: skin.palette.text_dim,
            metrics: skin.tab_large,
            role: TextRoleSkin {
                color: ColorRole::Text,
                font: FontFamily::Mono,
                size: skin.tab_large.text_size,
                spacing: 0.0,
                weight: FontWeight::Normal,
            },
            underline: skin.palette.accent,
        }
    }

    pub(crate) fn measure(&self, text: &mut TextContext, label: &str) -> (f32, f32) {
        let run = self.shape(text, label);
        (run.width() + self.metrics.pad_x * 2.0, self.metrics.height)
    }

    pub(crate) fn paint(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        label: &str,
        active: bool,
        bounds: Rect,
    ) {
        let run = self.shape(text, label);
        let label_height =
            (bounds.h - self.metrics.pad_y * 2.0 - self.metrics.underline_width).max(0.0);
        list.text(
            &run,
            label,
            Transform::translate(Pt {
                x: bounds.x + self.metrics.pad_x,
                y: bounds.y + self.metrics.pad_y + (label_height - run.height()) / 2.0,
            }),
            if active {
                self.active_color
            } else {
                self.idle_color
            },
        );
        if active {
            list.fill_rect(
                Rect {
                    h: self.metrics.underline_width,
                    w: (bounds.w - self.metrics.pad_x * 2.0).max(0.0),
                    x: bounds.x + self.metrics.pad_x,
                    y: bounds.y + bounds.h - self.metrics.pad_y - self.metrics.underline_width,
                },
                self.underline,
            );
        }
    }

    fn shape(&self, text: &mut TextContext, label: &str) -> GlyphRun {
        text.shape(label, self.role, None)
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        builtin,
        draw::{DrawCmd, Geom, Paint},
    };

    #[kithara::test]
    fn shaped_width_stays_equal_to_the_iced_tab_width() {
        let skin = builtin::skin();
        let mut text = TextContext::from(skin.text_resources());
        let (width, height) = TabLarge::new(skin).measure(&mut text, "DECK MICRO");

        assert!(
            (width - 94.0).abs() < 0.001,
            "iced measured this label and skin at 94 px; a {width} px shaped width would resize \
             the tab when its module becomes hosted"
        );
        assert_eq!(height, 28.0);
    }

    #[kithara::test]
    fn only_an_active_tab_draws_the_underline() {
        let skin = builtin::skin();
        let bounds = Rect {
            h: skin.tab_large.height,
            w: 94.0,
            x: 3.0,
            y: 5.0,
        };
        let tab = TabLarge::new(skin);
        let draw = |active| {
            let mut text = TextContext::from(skin.text_resources());
            let mut builder = DrawListBuilder::default();
            tab.paint(&mut builder, &mut text, "DECK MICRO", active, bounds);
            builder.finish()
        };
        let active = draw(true);
        let inactive = draw(false);

        let [label, underline] = active.commands() else {
            panic!("an active tab must draw its label followed by its underline");
        };
        assert!(matches!(
            label,
            DrawCmd::Text { content, color, .. }
                if content == "DECK MICRO" && *color == skin.palette.text
        ));
        assert!(matches!(
            underline,
            DrawCmd::Fill {
                geom: Geom::Rect(Rect {
                    h: 2.0,
                    w: 66.0,
                    x: 17.0,
                    y: 31.0,
                }),
                paint: Paint::Solid(color),
            } if *color == skin.palette.accent
        ));
        assert!(matches!(
            inactive.commands(),
            [DrawCmd::Text { content, color, .. }]
                if content == "DECK MICRO" && *color == skin.palette.text_dim
        ));
    }
}
