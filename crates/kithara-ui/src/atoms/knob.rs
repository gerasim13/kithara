use crate::{
    draw::{DrawListBuilder, Pt, Rect, Rgba, Transform},
    render::Skin,
    skin::ColorRole,
    text::TextContext,
};

pub(crate) struct Knob<'data, 'skin> {
    label: Option<&'data str>,
    value: f32,
    skin: &'skin Skin,
}

impl<'data, 'skin> Knob<'data, 'skin> {
    pub(crate) fn new(label: Option<&'data str>, value: f32, skin: &'skin Skin) -> Self {
        Self { label, value, skin }
    }

    fn color(&self, role: ColorRole) -> Rgba {
        self.skin.rgba(role)
    }

    pub(crate) fn paint(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        dial: Rect,
        caption: Rect,
    ) {
        let metrics = self.skin.knob;
        let side = dial.w.min(dial.h);
        let radius = side / 2.0;
        let center = Pt {
            x: dial.x + dial.w / 2.0,
            y: dial.y + dial.h / 2.0,
        };
        let angle = metrics.start_angle + metrics.sweep_angle * self.value;

        if radius > 0.0 {
            let track = self.color(metrics.track_color);
            list.stroke_arc(
                center,
                radius,
                metrics.start_angle,
                metrics.start_angle + metrics.sweep_angle,
                Rgba {
                    a: metrics.track_alpha,
                    ..track
                },
                metrics.track_width,
            );
            list.stroke_arc(
                center,
                radius,
                metrics.neutral_angle,
                angle,
                self.color(metrics.value_color),
                metrics.track_width,
            );

            let body_radius = metrics.body_ratio * radius;
            list.fill_circle(center, body_radius, self.color(metrics.body_fill));
            list.stroke_circle(
                center,
                body_radius,
                self.color(metrics.body_border),
                metrics.body_border_width,
            );
            list.stroke_line(
                center,
                Pt {
                    x: center.x + angle.cos() * body_radius,
                    y: center.y + angle.sin() * body_radius,
                },
                self.color(metrics.indicator_color),
                metrics.indicator_width,
            );
        }

        if let Some(label) = self.label {
            let role = metrics.label_text;
            let run = text.shape(label, role, Some(caption.w));
            list.text(
                &run,
                label,
                Transform::translate(Pt {
                    x: caption.x + (caption.w - run.width()) / 2.0,
                    y: caption.y,
                }),
                self.color(role.color),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        builtin,
        draw::{DrawCmd, DrawList, Geom},
        ids::SourceUri,
    };

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Kind {
        FillCircle,
        StrokeCircle,
        StrokeArc,
        StrokeLine,
        Text,
        Other,
    }

    #[kithara::test]
    fn knob_emits_ordered_commands_with_optional_text() {
        let origin = SourceUri("knob.kskin.ron".to_owned());
        let skin = Skin::resolve(builtin::skin_doc().clone(), &origin).unwrap();
        let labelled = record(Some("GAIN"), &skin);
        let unlabelled = record(None, &skin);

        assert_eq!(
            kinds(&labelled),
            [
                Kind::StrokeArc,
                Kind::StrokeArc,
                Kind::FillCircle,
                Kind::StrokeCircle,
                Kind::StrokeLine,
                Kind::Text,
            ]
        );
        assert_eq!(
            kinds(&unlabelled),
            [
                Kind::StrokeArc,
                Kind::StrokeArc,
                Kind::FillCircle,
                Kind::StrokeCircle,
                Kind::StrokeLine,
            ]
        );
        assert_eq!(
            &labelled.commands()[..labelled.commands().len() - 1],
            unlabelled.commands(),
            "the dial recording must not depend on caption presence"
        );
        assert!(matches!(
            labelled.commands().last(),
            Some(DrawCmd::Text { content, .. }) if content == "GAIN"
        ));
    }

    fn record(label: Option<&str>, skin: &Skin) -> DrawList {
        const DIAL: Rect = Rect {
            h: 22.0,
            w: 22.0,
            x: 3.0,
            y: 3.0,
        };
        const CAPTION: Rect = Rect {
            h: 9.0,
            w: 28.0,
            x: 0.0,
            y: 30.0,
        };

        let knob = Knob::new(label, 0.25, skin);
        let mut list = DrawListBuilder::default();
        knob.paint(&mut list, &mut TextContext::new().unwrap(), DIAL, CAPTION);
        list.finish()
    }

    fn kinds(list: &DrawList) -> Vec<Kind> {
        let commands = list.commands();
        commands
            .iter()
            .map(|command| match command {
                DrawCmd::Fill {
                    geom: Geom::Circle { .. },
                    ..
                } => Kind::FillCircle,
                DrawCmd::Stroke {
                    geom: Geom::Circle { .. },
                    ..
                } => Kind::StrokeCircle,
                DrawCmd::Stroke {
                    geom: Geom::Arc { .. },
                    ..
                } => Kind::StrokeArc,
                DrawCmd::Stroke {
                    geom: Geom::Line { .. },
                    ..
                } => Kind::StrokeLine,
                DrawCmd::Text { .. } => Kind::Text,
                _ => Kind::Other,
            })
            .collect()
    }
}
