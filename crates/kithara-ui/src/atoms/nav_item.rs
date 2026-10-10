use crate::{
    atoms::{icon::mark::Marked, painter::NavData},
    draw::{DrawListBuilder, Pt, Rect, Rgba, TRANSPARENT, Transform},
    module::TextStyle,
    render::Skin,
    shaping::TextContext,
    skin::{NavSkin, TextRoleSkin},
};

#[derive(Clone, PartialEq, kithara_derive::ControlPainter)]
#[control_painter(
    data = NavData,
    draw = self.paint(list, text, data, bounds)
)]
pub(crate) struct NavItem {
    active: Face,
    idle: Face,
    metrics: NavSkin,
    role: TextRoleSkin,
}

/// How the item looks in one of its two states. Both are resolved from the skin
/// when the item is built, so the page it points at becoming the current one is
/// a paint-time choice rather than a reason to rebuild the control.
#[derive(Clone, PartialEq)]
struct Face {
    background: Rgba,
    content: Rgba,
    icon: Rgba,
    marker: Rgba,
}

impl NavItem {
    pub(crate) fn new(skin: &Skin, style: Option<TextStyle>) -> Self {
        Self {
            active: Face {
                background: skin.rgba(skin.nav.selected_fill),
                content: skin.rgba(skin.nav.text.color),
                icon: skin.rgba(skin.nav.text.color),
                marker: skin.rgba(skin.nav.marker_color),
            },
            idle: Face {
                background: TRANSPARENT,
                content: skin.rgba(skin.nav.idle_text_color),
                icon: skin.rgba(skin.nav.idle_icon_color),
                marker: TRANSPARENT,
            },
            metrics: skin.nav,
            role: style.map_or(skin.nav.text, |style| skin.text.role(style)),
        }
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

    pub(crate) fn paint(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        data: &NavData,
        bounds: Rect,
    ) {
        let face = if data.active {
            &self.active
        } else {
            &self.idle
        };
        let marker = self.marker(bounds);
        list.fill_rect(bounds, face.background);
        list.fill_rect(marker, face.marker);
        self.paint_content(list, text, data, face, bounds, marker);
    }

    fn paint_content(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        data: &NavData,
        face: &Face,
        bounds: Rect,
        marker: Rect,
    ) {
        let x = marker.x + marker.w + self.metrics.text_pad_x;
        let icon = Rect {
            x,
            w: self.metrics.icon_box,
            ..bounds
        };
        Marked::new(data.mark, self.metrics.icon_size).centred(list, text, icon, face.icon);

        if data.label.is_empty() {
            return;
        }
        let label = self.role.cased(&data.label);
        let run = text.shape(&label, self.role, None);
        list.text(
            &run,
            &label,
            Transform::translate(Pt {
                x: x + self.metrics.icon_box + self.metrics.icon_gap,
                y: bounds.y + (bounds.h - run.height()) / 2.0,
            }),
            face.content,
        );
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        builtin,
        draw::{DrawCmd, Geom, Paint},
        hosts::icons::Mark,
        shaping::{FontId, GlyphFace, GlyphSegment},
    };

    fn data(mark: Mark, active: bool) -> NavData {
        NavData {
            active,
            mark,
            label: "PRIMITIVES".to_owned(),
        }
    }

    #[kithara::test]
    fn an_active_nav_item_draws_its_background_marker_icon_and_label_in_order() {
        let skin = builtin::skin();
        let bounds = Rect {
            h: 30.0,
            w: 198.0,
            x: 3.0,
            y: 5.0,
        };
        let mark = Mark::Glyph(char::from(lucide_icons::Icon::Disc));
        let mut text = TextContext::from(skin.text_resources.as_ref());
        let mut builder = DrawListBuilder::default();
        NavItem::new(skin, None).paint(&mut builder, &mut text, &data(mark, true), bounds);
        let list = builder.finish();

        let [background, marker, icon, label] = list.commands() else {
            panic!("an active nav item must emit four drawing commands");
        };
        assert!(matches!(
            background,
            DrawCmd::Fill {
                geom: Geom::Rect(rect),
                paint: Paint::Solid(color),
            } if *rect == bounds && *color == skin.palette.bg_select
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
                paint: Paint::Solid(color),
            } if *color == skin.palette.accent
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
        let Mark::Glyph(glyph) = mark else {
            panic!("the fixture mark is a glyph");
        };
        assert_eq!(icon_content, &glyph.to_string());
        assert_eq!(icon_transform.dx, 17.0 + (14.0 - icon_run.width()) / 2.0);
        assert_eq!(
            icon_transform.dy,
            bounds.y + (bounds.h - icon_run.height()) / 2.0
        );
        assert_eq!(*icon_color, skin.palette.text);

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
        assert_eq!(label_transform.dx, 17.0 + 14.0 + 8.0);
        assert_eq!(
            label_transform.dy,
            bounds.y + (bounds.h - label_run.height()) / 2.0
        );
        assert_eq!(*label_color, skin.palette.text);
    }

    #[kithara::test]
    fn an_inactive_nav_item_keeps_both_rectangles_clear_and_dims_its_content() {
        let skin = builtin::skin();
        let mut text = TextContext::from(skin.text_resources.as_ref());
        let mut builder = DrawListBuilder::default();
        NavItem::new(skin, None).paint(
            &mut builder,
            &mut text,
            &data(Mark::Glyph(char::from(lucide_icons::Icon::Disc)), false),
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
                paint: Paint::Solid(background),
                ..
            },
            DrawCmd::Fill {
                paint: Paint::Solid(marker),
                ..
            },
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
        assert_eq!(*icon_color, skin.palette.icon);
        assert_eq!(*label_color, skin.palette.text_dim);
    }

    /// A nav item given a text role sets its label in that role's face and
    /// case.
    #[kithara::test]
    fn a_nav_item_given_a_text_role_sets_its_label_in_that_role() {
        let skin = builtin::skin();
        let mut text = TextContext::from(skin.text_resources.as_ref());
        let mut label = |style| {
            let mut builder = DrawListBuilder::default();
            NavItem::new(skin, style).paint(
                &mut builder,
                &mut text,
                &NavData {
                    active: true,
                    mark: Mark::Glyph(char::from(lucide_icons::Icon::Disc)),
                    label: "Zvuk".to_owned(),
                },
                Rect {
                    h: 30.0,
                    w: 198.0,
                    x: 0.0,
                    y: 0.0,
                },
            );
            let list = builder.finish();
            let [.., DrawCmd::Text { content, run, .. }] = list.commands() else {
                panic!("a nav item must end with its label");
            };
            (content.clone(), run.width())
        };
        let ((content, cell), (_, default)) = (label(Some(TextStyle::Cell)), label(None));

        assert_eq!(content, "ZVUK");
        assert_eq!(cell, text.shape("ZVUK", skin.text.cell, None).width());
        assert_ne!(cell, default);
    }

    /// Turning to another page is a repaint, not a rebuild, so one mounted item
    /// must be able to draw both states.
    #[kithara::test]
    fn one_nav_item_draws_both_states() {
        let skin = builtin::skin();
        let bounds = Rect {
            h: 30.0,
            w: 198.0,
            x: 0.0,
            y: 0.0,
        };
        let item = NavItem::new(skin, None);
        let mark = Mark::Glyph(char::from(lucide_icons::Icon::Disc));
        let mut text = TextContext::from(skin.text_resources.as_ref());
        let mut draw = |active| {
            let mut builder = DrawListBuilder::default();
            item.paint(&mut builder, &mut text, &data(mark, active), bounds);
            builder.finish()
        };

        assert_ne!(
            draw(true),
            draw(false),
            "the same nav item must draw differently once it is told it is current"
        );
    }
}
