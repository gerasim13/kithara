use crate::{
    draw::{DrawListBuilder, Pt, Rect, Rgba, Transform},
    layout::FrameSides,
    module::ButtonStyle,
    render::Skin,
    skin::{ColorRole, FontFamily, FontSkin, FrameSkin, TextRoleSkin},
    text::{GlyphRun, TextContext},
};

#[derive(bon::Builder)]
pub(crate) struct Button<'data, 'skin> {
    active: bool,
    active_label: Option<&'data str>,
    frame: Option<FrameSides>,
    glyph: Option<char>,
    label: &'data str,
    style: ButtonStyle,
    skin: &'skin Skin,
}

#[derive(Clone, Copy)]
pub(crate) enum VisualState {
    Idle,
    Hovered,
    Pressed,
}

impl Button<'_, '_> {
    pub(crate) fn paint(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        bounds: Rect,
        state: VisualState,
    ) {
        let (fill, content) = self.colors(state);
        let radius = self.frame_skin().map_or(0.0, |frame| frame.radius);
        list.fill_rounded_rect(bounds, radius, fill);
        if self.is_transport() {
            self.paint_transport_frame(list, bounds);
        } else if let Some(frame) = self.frame_skin() {
            self.paint_border(list, bounds, frame);
        }
        self.paint_content(list, text, bounds, content);
    }

    pub(crate) fn intrinsic_width(&self, text: &mut TextContext) -> f32 {
        let label = self.label();
        let content = match self.glyph {
            Some(glyph) => {
                let glyph = glyph.to_string();
                let icon = text.shape_lucide(&glyph, self.icon_size());
                let label_width = self.shape_label(text, label).width();
                let icon_only = self.style == ButtonStyle::MicroPrimary
                    || self.is_transport() && label.is_empty();
                icon.width()
                    + if icon_only {
                        0.0
                    } else {
                        self.skin.button.icon_gap + label_width
                    }
            }
            None if label.is_empty() => 0.0,
            None => self.shape_label(text, label).width(),
        };
        let padding = if self.style == ButtonStyle::VisNav {
            self.skin.vis.nav_padding_x
        } else {
            self.skin.button.padding_x
        };
        content + padding * 2.0
    }

    fn colors(&self, state: VisualState) -> (Rgba, Rgba) {
        let highlighted = self.highlighted();
        let palette = self.skin.palette;
        let fill: Rgba = if self.style == ButtonStyle::VisNav {
            match state {
                VisualState::Hovered => palette.bg_select.into(),
                VisualState::Pressed => palette.accent_soft.into(),
                VisualState::Idle => self.skin.rgba(self.skin.vis.nav_background),
            }
        } else if highlighted {
            match state {
                VisualState::Hovered => palette.accent_strong.into(),
                VisualState::Pressed => palette.accent_soft.into(),
                VisualState::Idle => palette.accent.into(),
            }
        } else if self.is_transport() {
            match state {
                VisualState::Hovered => palette.bg_panel_2.into(),
                VisualState::Pressed => palette.accent_soft.into(),
                VisualState::Idle => Rgba {
                    a: 0.0,
                    b: 0.0,
                    g: 0.0,
                    r: 0.0,
                },
            }
        } else {
            match state {
                VisualState::Hovered => palette.bg_panel_2.into(),
                VisualState::Pressed => palette.accent_soft.into(),
                VisualState::Idle => palette.bg_panel.into(),
            }
        };
        let content: Rgba = if self.style == ButtonStyle::VisNav {
            self.skin.rgba(self.skin.vis.nav_text_color)
        } else if highlighted {
            palette.bg.into()
        } else {
            palette.text.into()
        };
        (fill, content)
    }

    fn frame_skin(&self) -> Option<FrameSkin> {
        if self.is_transport() {
            None
        } else if self.style == ButtonStyle::VisNav {
            Some(self.skin.vis.nav_frame)
        } else if self.is_primary() {
            Some(self.skin.button.primary_frame)
        } else {
            Some(self.skin.button.frame)
        }
    }

    fn is_primary(&self) -> bool {
        matches!(
            self.style,
            ButtonStyle::TransportPrimary | ButtonStyle::MicroPrimary
        )
    }

    fn is_transport(&self) -> bool {
        matches!(
            self.style,
            ButtonStyle::Transport | ButtonStyle::TransportPrimary
        )
    }

    fn highlighted(&self) -> bool {
        self.active || self.style == ButtonStyle::MicroPrimary
    }

    fn paint_border(&self, list: &mut DrawListBuilder, bounds: Rect, frame: FrameSkin) {
        if frame.border_width <= 0.0 {
            return;
        }
        let inset = frame.border_width / 2.0;
        list.stroke_rounded_rect(
            Rect {
                h: (bounds.h - frame.border_width).max(0.0),
                w: (bounds.w - frame.border_width).max(0.0),
                x: bounds.x + inset,
                y: bounds.y + inset,
            },
            frame.radius,
            self.skin.rgba(frame.border),
            frame.border_width,
        );
    }

    fn paint_transport_frame(&self, list: &mut DrawListBuilder, bounds: Rect) {
        let sides = self.frame.unwrap_or(self.skin.button.transport_sides);
        let width = self.skin.divider.width.min(bounds.w).min(bounds.h);
        if width <= 0.0 {
            return;
        }
        let color = self.skin.rgba(self.skin.divider.color);
        if sides.top {
            list.fill_rect(Rect { h: width, ..bounds }, color);
        }
        if sides.right {
            list.fill_rect(
                Rect {
                    h: bounds.h,
                    w: width,
                    x: bounds.x + bounds.w - width,
                    y: bounds.y,
                },
                color,
            );
        }
        if sides.bottom {
            list.fill_rect(
                Rect {
                    h: width,
                    w: bounds.w,
                    x: bounds.x,
                    y: bounds.y + bounds.h - width,
                },
                color,
            );
        }
        if sides.left {
            list.fill_rect(
                Rect {
                    h: bounds.h,
                    w: width,
                    x: bounds.x,
                    y: bounds.y,
                },
                color,
            );
        }
    }

    fn paint_content(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        bounds: Rect,
        color: Rgba,
    ) {
        let label = self.label();
        let Some(glyph) = self.glyph else {
            self.paint_label(list, text, label, bounds, color);
            return;
        };
        let icon_size = self.icon_size();
        let glyph = glyph.to_string();
        let icon = text.shape_lucide(&glyph, icon_size);
        if self.style == ButtonStyle::MicroPrimary || self.is_transport() && label.is_empty() {
            let icon_color = if self.is_transport() && !self.highlighted() {
                self.skin.palette.text_dim.into()
            } else {
                color
            };
            Self::paint_run(list, &icon, &glyph, bounds, icon_color);
            return;
        }

        let label_run = self.shape_label(text, label);
        let width = icon.width() + self.skin.button.icon_gap + label_run.width();
        let x = bounds.x + (bounds.w - width) / 2.0;
        list.text(
            &icon,
            &glyph,
            Transform::translate(Pt {
                x,
                y: bounds.y + (bounds.h - icon.height()) / 2.0,
            }),
            color,
        );
        if !label.is_empty() {
            list.text(
                &label_run,
                label,
                Transform::translate(Pt {
                    x: x + icon.width() + self.skin.button.icon_gap,
                    y: bounds.y + (bounds.h - label_run.height()) / 2.0,
                }),
                color,
            );
        }
    }

    fn paint_label(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        label: &str,
        bounds: Rect,
        color: Rgba,
    ) {
        if label.is_empty() {
            return;
        }
        let run = self.shape_label(text, label);
        Self::paint_run(list, &run, label, bounds, color);
    }

    fn paint_run(
        list: &mut DrawListBuilder,
        run: &GlyphRun,
        content: &str,
        bounds: Rect,
        color: Rgba,
    ) {
        list.text(
            run,
            content,
            Transform::translate(Pt {
                x: bounds.x + (bounds.w - run.width()) / 2.0,
                y: bounds.y + (bounds.h - run.height()) / 2.0,
            }),
            color,
        );
    }

    fn shape_label(&self, text: &mut TextContext, label: &str) -> GlyphRun {
        let font = self.font();
        text.shape(
            label,
            TextRoleSkin {
                color: ColorRole::Text,
                font: FontFamily::Mono,
                size: font.size,
                spacing: 0.0,
                weight: font.weight,
            },
            None,
        )
    }

    fn label(&self) -> &str {
        if self.active {
            self.active_label.unwrap_or(self.label)
        } else {
            self.label
        }
    }

    fn font(&self) -> FontSkin {
        if self.is_primary() || self.active {
            self.skin.button.primary_text
        } else if self.style == ButtonStyle::VisNav {
            self.skin.vis.nav_text
        } else {
            self.skin.button.text
        }
    }

    fn icon_size(&self) -> f32 {
        if self.style == ButtonStyle::MicroPrimary {
            self.skin.button.micro_icon_size
        } else if self.is_transport() {
            self.skin.button.transport_icon_size
        } else {
            self.skin.button.icon_size
        }
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        builtin,
        draw::{DrawCmd, DrawListBuilder, Geom, Rect},
        module::ButtonStyle,
        text::{FontId, GlyphFace, GlyphSegment, TextContext},
    };

    #[kithara::test]
    fn a_default_button_draws_fill_border_and_label_in_order() {
        let skin = builtin::skin();
        let bounds = Rect {
            h: 30.0,
            w: 72.0,
            x: 0.0,
            y: 0.0,
        };
        let mut text = TextContext::from(skin.text_resources());
        let mut builder = DrawListBuilder::default();
        Button::builder()
            .active(false)
            .label("DEFAULT")
            .style(ButtonStyle::Default)
            .skin(skin)
            .build()
            .paint(&mut builder, &mut text, bounds, VisualState::Idle);
        let list = builder.finish();

        assert_eq!(list.commands().len(), 3);
        assert!(matches!(
            list.commands()[0],
            DrawCmd::Fill {
                geom: Geom::Rect(rect),
                color,
            } if rect == bounds && color == skin.palette.bg_panel.into()
        ));
        assert!(matches!(
            list.commands()[1],
            DrawCmd::Stroke {
                geom: Geom::Rect(_),
                color,
                width: 1.0,
            } if color == skin.palette.line.into()
        ));
        assert!(matches!(
            &list.commands()[2],
            DrawCmd::Text { run, content, .. }
                if content == "DEFAULT"
                    && run.segments().first().map(GlyphSegment::face)
                        == Some(&GlyphFace::Embedded(FontId::JetBrainsMonoRegular))
        ));
    }

    #[kithara::test]
    fn a_micro_button_draws_its_lucide_glyph_through_the_text_command() {
        let skin = builtin::skin();
        let glyph = char::from(lucide_icons::Icon::Play);
        let mut text = TextContext::from(skin.text_resources());
        let mut builder = DrawListBuilder::default();
        Button::builder()
            .active(false)
            .glyph(glyph)
            .label("PLAY")
            .style(ButtonStyle::MicroPrimary)
            .skin(skin)
            .build()
            .paint(
                &mut builder,
                &mut text,
                Rect {
                    h: 34.0,
                    w: 34.0,
                    x: 0.0,
                    y: 0.0,
                },
                VisualState::Idle,
            );
        let list = builder.finish();

        assert!(matches!(
            &list.commands()[2],
            DrawCmd::Text { run, content, .. }
                if content == &glyph.to_string()
                    && run.segments().first().map(GlyphSegment::face)
                        == Some(&GlyphFace::Embedded(FontId::Lucide))
        ));
    }

    #[kithara::test]
    fn a_transport_button_draws_only_its_declared_seams() {
        let skin = builtin::skin();
        let bounds = Rect {
            h: 28.0,
            w: 48.0,
            x: 0.0,
            y: 0.0,
        };
        let mut text = TextContext::from(skin.text_resources());
        let mut builder = DrawListBuilder::default();
        Button::builder()
            .active(false)
            .frame(FrameSides {
                top: true,
                right: false,
                bottom: true,
                left: false,
            })
            .label("PLAY")
            .style(ButtonStyle::TransportPrimary)
            .skin(skin)
            .build()
            .paint(&mut builder, &mut text, bounds, VisualState::Idle);
        let list = builder.finish();

        assert!(matches!(
            list.commands()[0],
            DrawCmd::Fill {
                geom: Geom::Rect(rect),
                color,
            } if rect == bounds && color.a == 0.0
        ));
        assert!(matches!(
            list.commands()[1],
            DrawCmd::Fill {
                geom: Geom::Rect(Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 48.0,
                    h: 1.0
                }),
                ..
            }
        ));
        assert!(matches!(
            list.commands()[2],
            DrawCmd::Fill {
                geom: Geom::Rect(Rect {
                    x: 0.0,
                    y: 27.0,
                    w: 48.0,
                    h: 1.0
                }),
                ..
            }
        ));
        assert!(matches!(list.commands()[3], DrawCmd::Text { .. }));
    }

    #[kithara::test]
    fn an_active_transport_button_uses_its_accent_and_active_label() {
        let skin = builtin::skin();
        let mut text = TextContext::from(skin.text_resources());
        let mut builder = DrawListBuilder::default();
        Button::builder()
            .active(true)
            .active_label("PAUSE")
            .label("PLAY")
            .style(ButtonStyle::TransportPrimary)
            .skin(skin)
            .build()
            .paint(
                &mut builder,
                &mut text,
                Rect {
                    h: 28.0,
                    w: 48.0,
                    x: 0.0,
                    y: 0.0,
                },
                VisualState::Idle,
            );
        let list = builder.finish();

        assert!(matches!(
            list.commands()[0],
            DrawCmd::Fill { color, .. } if color == skin.palette.accent.into()
        ));
        assert!(matches!(
            list.commands().last(),
            Some(DrawCmd::Text { content, color, .. })
                if content == "PAUSE" && *color == skin.palette.bg.into()
        ));
    }
}
