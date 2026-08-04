use crate::{
    draw::{DrawListBuilder, Pt, Rect, Rgba, Transform},
    render::Skin,
    skin::{ColorRole, FontFamily, FontWeight, NavSkin, TextRoleSkin},
    text::TextContext,
};

pub(crate) struct NavItem {
    background: Rgba,
    content: Rgba,
    glyph: char,
    marker_color: Rgba,
    metrics: NavSkin,
    role: TextRoleSkin,
}

impl NavItem {
    pub(crate) fn new(glyph: char, active: bool, skin: &Skin) -> Self {
        let transparent = Rgba {
            a: 0.0,
            b: 0.0,
            g: 0.0,
            r: 0.0,
        };
        Self {
            background: if active {
                skin.palette.bg_select.into()
            } else {
                transparent
            },
            content: if active {
                skin.palette.text.into()
            } else {
                skin.palette.text_dim.into()
            },
            glyph,
            marker_color: if active {
                skin.palette.accent.into()
            } else {
                transparent
            },
            metrics: skin.nav,
            role: TextRoleSkin {
                color: ColorRole::Text,
                font: FontFamily::Mono,
                size: skin.nav.text_size,
                spacing: 0.0,
                weight: FontWeight::Normal,
            },
        }
    }

    pub(crate) fn paint(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        label: &str,
        bounds: Rect,
    ) {
        let marker = self.marker(bounds);
        list.fill_rect(bounds, self.background);
        list.fill_rect(marker, self.marker_color);
        self.paint_content(list, text, label, bounds, marker);
    }

    fn marker(&self, bounds: Rect) -> Rect {
        let padding = self.metrics.pad_y;
        let inner_width = (bounds.w - padding * 2.0).max(0.0);
        Rect {
            h: (bounds.h - padding * 2.0).max(0.0),
            w: self.metrics.marker_width.max(0.0).min(inner_width),
            x: bounds.x + padding,
            y: bounds.y + padding,
        }
    }

    fn paint_content(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        label: &str,
        bounds: Rect,
        marker: Rect,
    ) {
        let glyph = self.glyph.to_string();
        let icon = text.shape_lucide(&glyph, self.metrics.icon_size);
        let x = marker.x + marker.w + self.metrics.text_pad_x;
        list.text(
            &icon,
            &glyph,
            Transform::translate(Pt {
                x,
                y: bounds.y + (bounds.h - icon.height()) / 2.0,
            }),
            self.content,
        );

        if label.is_empty() {
            return;
        }
        let run = text.shape(label, self.role, None);
        list.text(
            &run,
            label,
            Transform::translate(Pt {
                x: x + icon.width() + self.metrics.icon_gap,
                y: bounds.y + (bounds.h - run.height()) / 2.0,
            }),
            self.content,
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
        text::{FontId, GlyphFace, GlyphSegment},
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
        NavItem::new(glyph, true, skin).paint(&mut builder, &mut text, "PRIMITIVES", bounds);
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
            Some(&GlyphFace::Embedded(FontId::Lucide))
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
            Some(&GlyphFace::Embedded(FontId::JetBrainsMonoRegular))
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
        NavItem::new(char::from(lucide_icons::Icon::Disc), false, skin).paint(
            &mut builder,
            &mut text,
            "PRIMITIVES",
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
