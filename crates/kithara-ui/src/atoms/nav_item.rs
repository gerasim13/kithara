use crate::{
    draw::{DrawListBuilder, Pt, Rect, Rgba, Transform},
    render::Skin,
    skin::{ColorRole, FontFamily, FontWeight, TextRoleSkin},
    text::TextContext,
};

#[derive(bon::Builder)]
pub(crate) struct NavItem<'data, 'skin> {
    active: bool,
    glyph: char,
    label: &'data str,
    skin: &'skin Skin,
}

impl NavItem<'_, '_> {
    pub(crate) fn paint(&self, list: &mut DrawListBuilder, text: &mut TextContext, bounds: Rect) {
        let transparent = Rgba {
            a: 0.0,
            b: 0.0,
            g: 0.0,
            r: 0.0,
        };
        let background = if self.active {
            self.skin.palette.bg_select.into()
        } else {
            transparent
        };
        let marker_color = if self.active {
            self.skin.palette.accent.into()
        } else {
            transparent
        };
        let content_color = if self.active {
            self.skin.palette.text.into()
        } else {
            self.skin.palette.text_dim.into()
        };
        let marker = self.marker(bounds);

        list.fill_rect(bounds, background);
        list.fill_rect(marker, marker_color);
        self.paint_content(list, text, bounds, marker, content_color);
    }

    fn marker(&self, bounds: Rect) -> Rect {
        let padding = self.skin.nav.pad_y;
        let inner_width = (bounds.w - padding * 2.0).max(0.0);
        Rect {
            h: (bounds.h - padding * 2.0).max(0.0),
            w: self.skin.nav.marker_width.max(0.0).min(inner_width),
            x: bounds.x + padding,
            y: bounds.y + padding,
        }
    }

    fn paint_content(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        bounds: Rect,
        marker: Rect,
        color: Rgba,
    ) {
        let glyph = self.glyph.to_string();
        let icon = text.shape_lucide(&glyph, self.skin.nav.icon_size);
        let x = marker.x + marker.w + self.skin.nav.text_pad_x;
        list.text(
            &icon,
            &glyph,
            Transform::translate(Pt {
                x,
                y: bounds.y + (bounds.h - icon.height()) / 2.0,
            }),
            color,
        );

        if self.label.is_empty() {
            return;
        }
        let label = text.shape(
            self.label,
            TextRoleSkin {
                color: ColorRole::Text,
                font: FontFamily::Mono,
                size: self.skin.nav.text_size,
                spacing: 0.0,
                weight: FontWeight::Normal,
            },
            None,
        );
        list.text(
            &label,
            self.label,
            Transform::translate(Pt {
                x: x + icon.width() + self.skin.nav.icon_gap,
                y: bounds.y + (bounds.h - label.height()) / 2.0,
            }),
            color,
        );
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        builtin,
        draw::{DrawCmd, Geom},
        text::{FontId, GlyphSegment},
    };

    #[kithara::test]
    fn an_active_nav_item_draws_its_background_marker_icon_and_label_in_order() {
        let skin = builtin::skin();
        let bounds = Rect {
            h: 30.0,
            w: 198.0,
            x: 3.0,
            y: 5.0,
        };
        let glyph = char::from(lucide_icons::Icon::Disc);
        let mut text = TextContext::from(skin.text_resources());
        let mut builder = DrawListBuilder::default();
        NavItem::builder()
            .active(true)
            .glyph(glyph)
            .label("PRIMITIVES")
            .skin(skin)
            .build()
            .paint(&mut builder, &mut text, bounds);
        let list = builder.finish();

        let [background, marker, icon, label] = list.commands() else {
            panic!("an active nav item must emit four drawing commands");
        };
        assert!(matches!(
            background,
            DrawCmd::Fill {
                geom: Geom::Rect(rect),
                color,
            } if *rect == bounds && *color == skin.palette.bg_select.into()
        ));
        assert!(matches!(
            marker,
            DrawCmd::Fill {
                geom: Geom::Rect(Rect {
                    h: 30.0,
                    w: 2.0,
                    x: 3.0,
                    y: 5.0,
                }),
                color,
            } if *color == skin.palette.accent.into()
        ));
        let DrawCmd::Text {
            run: icon_run,
            content: icon_content,
            transform: icon_transform,
            color: icon_color,
        } = icon
        else {
            panic!("the third command must draw the nav icon");
        };
        assert_eq!(
            icon_run.segments().first().map(GlyphSegment::face),
            Some(FontId::Lucide)
        );
        assert_eq!(icon_content, &glyph.to_string());
        assert_eq!(icon_transform.dx, 19.0);
        assert_eq!(
            icon_transform.dy,
            bounds.y + (bounds.h - icon_run.height()) / 2.0
        );
        assert_eq!(*icon_color, skin.palette.text.into());

        let DrawCmd::Text {
            run: label_run,
            content: label_content,
            transform: label_transform,
            color: label_color,
        } = label
        else {
            panic!("the fourth command must draw the nav label");
        };
        assert_eq!(
            label_run.segments().first().map(GlyphSegment::face),
            Some(FontId::JetBrainsMonoRegular)
        );
        assert_eq!(label_content, "PRIMITIVES");
        assert_eq!(
            label_transform.dx,
            icon_transform.dx + icon_run.width() + 8.0
        );
        assert_eq!(
            label_transform.dy,
            bounds.y + (bounds.h - label_run.height()) / 2.0
        );
        assert_eq!(*label_color, skin.palette.text.into());
    }

    #[kithara::test]
    fn an_inactive_nav_item_keeps_both_rectangles_clear_and_dims_its_content() {
        let skin = builtin::skin();
        let mut text = TextContext::from(skin.text_resources());
        let mut builder = DrawListBuilder::default();
        NavItem::builder()
            .active(false)
            .glyph(char::from(lucide_icons::Icon::Disc))
            .label("PRIMITIVES")
            .skin(skin)
            .build()
            .paint(
                &mut builder,
                &mut text,
                Rect {
                    h: 30.0,
                    w: 198.0,
                    x: 0.0,
                    y: 0.0,
                },
            );
        let list = builder.finish();
        let [
            DrawCmd::Fill {
                color: background, ..
            },
            DrawCmd::Fill { color: marker, .. },
            DrawCmd::Text {
                color: icon_color, ..
            },
            DrawCmd::Text {
                color: label_color, ..
            },
        ] = list.commands()
        else {
            panic!("an inactive nav item must emit two fills and two text commands");
        };

        assert_eq!(background.a, 0.0);
        assert_eq!(marker.a, 0.0);
        assert_eq!(*icon_color, skin.palette.text_dim.into());
        assert_eq!(*label_color, skin.palette.text_dim.into());
    }
}
