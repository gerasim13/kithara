use crate::{
    draw::{DrawListBuilder, Pt, Rect, Transform},
    render::Skin,
    skin::{ColorRole, FontFamily, FontWeight, TextRoleSkin},
    text::{GlyphRun, TextContext},
};

pub(crate) struct TabLarge<'data, 'skin> {
    active: bool,
    label: &'data str,
    skin: &'skin Skin,
}

impl<'data, 'skin> TabLarge<'data, 'skin> {
    pub(crate) const fn new(label: &'data str, active: bool, skin: &'skin Skin) -> Self {
        Self {
            active,
            label,
            skin,
        }
    }

    pub(crate) fn measure(&self, text: &mut TextContext) -> (f32, f32) {
        let run = self.shape(text);
        (
            run.width() + self.skin.tab_large.pad_x * 2.0,
            self.skin.tab_large.height,
        )
    }

    pub(crate) fn paint(&self, list: &mut DrawListBuilder, text: &mut TextContext, bounds: Rect) {
        let run = self.shape(text);
        let label_height =
            (bounds.h - self.skin.tab_large.pad_y * 2.0 - self.skin.tab_large.underline_width)
                .max(0.0);
        let color = if self.active {
            self.skin.palette.text
        } else {
            self.skin.palette.text_dim
        };
        list.text(
            &run,
            self.label,
            Transform::translate(Pt {
                x: bounds.x + self.skin.tab_large.pad_x,
                y: bounds.y + self.skin.tab_large.pad_y + (label_height - run.height()) / 2.0,
            }),
            color.into(),
        );
        if self.active {
            list.fill_rect(
                Rect {
                    h: self.skin.tab_large.underline_width,
                    w: (bounds.w - self.skin.tab_large.pad_x * 2.0).max(0.0),
                    x: bounds.x + self.skin.tab_large.pad_x,
                    y: bounds.y + bounds.h
                        - self.skin.tab_large.pad_y
                        - self.skin.tab_large.underline_width,
                },
                self.skin.palette.accent.into(),
            );
        }
    }

    fn shape(&self, text: &mut TextContext) -> GlyphRun {
        text.shape(
            self.label,
            TextRoleSkin {
                color: ColorRole::Text,
                font: FontFamily::Mono,
                size: self.skin.tab_large.text_size,
                spacing: 0.0,
                weight: FontWeight::Normal,
            },
            None,
        )
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        builtin,
        draw::{DrawCmd, Geom},
    };

    #[kithara::test]
    fn shaped_width_stays_equal_to_the_iced_tab_width() {
        let skin = builtin::skin();
        let mut text = TextContext::from(skin.text_resources());
        let (width, height) = TabLarge::new("DECK MICRO", true, skin).measure(&mut text);

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
        let draw = |active| {
            let mut text = TextContext::from(skin.text_resources());
            let mut builder = DrawListBuilder::default();
            TabLarge::new("DECK MICRO", active, skin).paint(&mut builder, &mut text, bounds);
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
                if content == "DECK MICRO" && *color == skin.palette.text.into()
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
                color,
            } if *color == skin.palette.accent.into()
        ));
        assert!(matches!(
            inactive.commands(),
            [DrawCmd::Text { content, color, .. }]
                if content == "DECK MICRO" && *color == skin.palette.text_dim.into()
        ));
    }
}
