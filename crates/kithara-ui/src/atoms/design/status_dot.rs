use crate::{
    atoms::design::quad::center_y,
    draw::{DrawListBuilder, Pt, Rect, Rgba, Transform},
    module::Tone,
    render::Skin,
    skin::{FontFamily, StatusDotSkin, TextRoleSkin},
    text::TextContext,
};

/// A coloured dot with a word beside it.
pub(crate) struct StatusDot {
    active_dot: Option<Rgba>,
    dot: Rgba,
    dot_size: f32,
    metrics: StatusDotSkin,
    role: TextRoleSkin,
    text: Rgba,
}

/// The caption and whether the document marks this dot active.
pub(crate) struct StatusDotData {
    pub(crate) active: bool,
    pub(crate) label: String,
}

impl StatusDot {
    pub(crate) fn with_active_tone(
        tone: Tone,
        active_tone: Option<Tone>,
        dot_size: Option<f32>,
        skin: &Skin,
    ) -> Self {
        let metrics = skin.status_dot;
        Self {
            active_dot: active_tone.map(|tone| color(tone, skin)),
            dot: color(tone, skin),
            dot_size: dot_size.unwrap_or(metrics.dot_size),
            metrics,
            role: TextRoleSkin {
                color: metrics.text_color,
                font: FontFamily::Mono,
                size: metrics.text.size,
                spacing: 0.0,
                weight: metrics.text.weight,
            },
            text: skin.rgba(metrics.text_color),
        }
    }

    pub(crate) fn paint_with_state(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        label: &str,
        bounds: Rect,
        active: bool,
    ) {
        let radius = self.dot_size / 2.0;
        list.fill_circle(
            Pt {
                x: bounds.x + radius,
                y: bounds.y + bounds.h / 2.0,
            },
            radius,
            self.active_dot.filter(|_| active).unwrap_or(self.dot),
        );
        if label.is_empty() {
            return;
        }
        let run = text.shape(label, self.role, None);
        list.text(
            &run,
            label,
            Transform::translate(Pt {
                x: bounds.x + self.dot_size + self.metrics.gap,
                y: center_y(bounds, &run),
            }),
            self.text,
        );
    }
}

fn color(tone: Tone, skin: &Skin) -> Rgba {
    match tone {
        Tone::Neutral => skin.palette.muted,
        Tone::Accent => skin.palette.accent,
        Tone::Success => skin.palette.success,
        Tone::Danger => skin.palette.danger,
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::{DrawListBuilder, Rect, StatusDot, TextContext, Tone};
    use crate::{
        builtin,
        draw::{DrawCmd, Geom, Paint},
    };

    /// The tone is the whole point of the control, and the caption clears the
    /// dot rather than sitting under it.
    #[kithara::test]
    fn the_dot_carries_the_tone_and_the_caption_clears_it() {
        let skin = builtin::skin();
        let bounds = Rect {
            h: 18.0,
            w: 64.0,
            x: 3.0,
            y: 5.0,
        };
        let draw = |tone| {
            let mut text = TextContext::from(skin.text_resources());
            let mut list = DrawListBuilder::default();
            StatusDot::with_active_tone(tone, None, None, skin)
                .paint_with_state(&mut list, &mut text, "LIVE", bounds, false);
            list.finish()
        };

        let list = draw(Tone::Danger);
        let [dot, caption] = list.commands() else {
            panic!("a status dot must draw its dot followed by its caption");
        };
        assert!(matches!(
            dot,
            DrawCmd::Fill {
                geom: Geom::Circle { center, radius },
                paint: Paint::Solid(color),
            } if *color == skin.palette.danger
                && *radius == skin.status_dot.dot_size / 2.0
                && center.x == bounds.x + *radius
                && center.y == bounds.y + bounds.h / 2.0
        ));
        assert!(matches!(
            caption,
            DrawCmd::Text {
                content, transform, ..
            } if content == "LIVE"
                && transform.dx == bounds.x + skin.status_dot.dot_size + skin.status_dot.gap
        ));

        assert_ne!(
            draw(Tone::Danger),
            draw(Tone::Success),
            "the tone must reach the dot"
        );

        let active = StatusDot::with_active_tone(Tone::Neutral, Some(Tone::Danger), None, skin);
        let mut text = TextContext::from(skin.text_resources());
        let mut list = DrawListBuilder::default();
        active.paint_with_state(&mut list, &mut text, "LIVE", bounds, true);
        assert!(matches!(
            list.finish().commands(),
            [DrawCmd::Fill { paint: Paint::Solid(color), .. }, ..] if *color == skin.palette.danger
        ));
    }
}
