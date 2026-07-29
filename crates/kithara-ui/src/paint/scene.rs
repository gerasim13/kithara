use kithara_platform::sync::Arc as SharedArc;
use vello::{
    Glyph, Scene,
    kurbo::{Affine, Arc, Cap, Circle, Join, Line, Point, Stroke, Vec2},
    peniko::{Blob, Color, Fill, FontData},
};

use super::{Painter, Pt, Rgba, Transform};
use crate::text::GlyphRun;

/// A [`Painter`] that encodes drawing commands into a Vello [`Scene`].
pub struct ScenePainter<'scene> {
    scene: &'scene mut Scene,
}

impl<'scene> ScenePainter<'scene> {
    /// Creates a painter for `scene`.
    pub const fn new(scene: &'scene mut Scene) -> Self {
        Self { scene }
    }
}

impl Painter for ScenePainter<'_> {
    fn fill_circle(&mut self, center: Pt, radius: f32, color: Rgba) {
        self.scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            Color::from(color),
            None,
            &Circle::new(center, f64::from(radius)),
        );
    }

    fn stroke_arc(
        &mut self,
        center: Pt,
        radius: f32,
        start: f32,
        end: f32,
        color: Rgba,
        width: f32,
    ) {
        let arc = Arc::new(
            center,
            Vec2::splat(f64::from(radius)),
            f64::from(start),
            f64::from(end - start),
            0.0,
        );
        self.scene.stroke(
            &stroke(width),
            Affine::IDENTITY,
            Color::from(color),
            None,
            &arc,
        );
    }

    fn stroke_circle(&mut self, center: Pt, radius: f32, color: Rgba, width: f32) {
        self.scene.stroke(
            &stroke(width),
            Affine::IDENTITY,
            Color::from(color),
            None,
            &Circle::new(center, f64::from(radius)),
        );
    }

    fn stroke_line(&mut self, from: Pt, to: Pt, color: Rgba, width: f32) {
        self.scene.stroke(
            &stroke(width),
            Affine::IDENTITY,
            Color::from(color),
            None,
            &Line::new(from, to),
        );
    }

    fn text(&mut self, run: &GlyphRun, _content: &str, transform: Transform, color: Rgba) {
        let data = FontData::new(Blob::new(SharedArc::new(run.font().bytes())), 0);
        let glyphs = run.glyphs().iter().map(|glyph| Glyph {
            id: glyph.id,
            x: glyph.x,
            y: glyph.y,
        });
        self.scene
            .draw_glyphs(&data)
            .transform(Affine::new([
                f64::from(transform.xx),
                f64::from(transform.yx),
                f64::from(transform.xy),
                f64::from(transform.yy),
                f64::from(transform.dx),
                f64::from(transform.dy),
            ]))
            .font_size(run.size())
            .brush(Color::from(color))
            .draw(Fill::NonZero, glyphs);
    }
}

fn stroke(width: f32) -> Stroke {
    Stroke::new(f64::from(width))
        .with_caps(Cap::Butt)
        .with_join(Join::Miter)
}

impl From<Rgba> for Color {
    fn from(color: Rgba) -> Self {
        Self::new([color.r, color.g, color.b, color.a])
    }
}

impl From<Pt> for Point {
    fn from(point: Pt) -> Self {
        Self::new(f64::from(point.x), f64::from(point.y))
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;
    #[cfg(feature = "render")]
    use crate::{
        atoms::knob::Knob,
        builtin,
        paint::record::{Cmd, RecordPainter},
    };
    use crate::{
        paint::Rect,
        skin::{FontFamily, FontWeight},
        text::TextContext,
    };

    #[kithara::test]
    fn every_painter_operation_adds_to_the_encoding() {
        let mut scene = Scene::new();

        let before = scene.encoding().n_paths;
        ScenePainter::new(&mut scene).fill_circle(FIXTURE.point, 5.0, FIXTURE.color);
        assert!(scene.encoding().n_paths > before);

        let before = scene.encoding().n_paths;
        ScenePainter::new(&mut scene).stroke_arc(FIXTURE.point, 5.0, 0.0, 1.0, FIXTURE.color, 1.0);
        assert!(scene.encoding().n_paths > before);

        let before = scene.encoding().n_paths;
        ScenePainter::new(&mut scene).stroke_circle(FIXTURE.point, 5.0, FIXTURE.color, 1.0);
        assert!(scene.encoding().n_paths > before);

        let before = scene.encoding().n_paths;
        ScenePainter::new(&mut scene).stroke_line(
            FIXTURE.point,
            Pt { x: 8.0, y: 8.0 },
            FIXTURE.color,
            1.0,
        );
        assert!(scene.encoding().n_paths > before);

        let before = scene.encoding().resources.glyphs.len();
        paint_text(&mut scene, "GAIN");
        assert!(scene.encoding().resources.glyphs.len() > before);
    }

    #[kithara::test]
    fn stroke_width_changes_the_encoding() {
        let mut thin = Scene::new();
        ScenePainter::new(&mut thin).stroke_line(
            FIXTURE.point,
            Pt { x: 8.0, y: 8.0 },
            FIXTURE.color,
            1.0,
        );
        let mut thick = Scene::new();
        ScenePainter::new(&mut thick).stroke_line(
            FIXTURE.point,
            Pt { x: 8.0, y: 8.0 },
            FIXTURE.color,
            3.0,
        );

        assert_ne!(thin.encoding().styles, thick.encoding().styles);
    }

    #[kithara::test]
    fn stroke_uses_butt_caps_and_miter_join() {
        let stroke = stroke(2.0);

        assert_eq!(stroke.start_cap, Cap::Butt);
        assert_eq!(stroke.end_cap, Cap::Butt);
        assert_eq!(stroke.join, Join::Miter);
    }

    #[kithara::test]
    fn text_content_adds_glyphs() {
        let no_text = text_scene("");
        let text = text_scene("GAIN");

        assert!(no_text.encoding().resources.glyphs.is_empty());
        assert!(!text.encoding().resources.glyphs.is_empty());
    }

    #[kithara::test]
    fn missing_character_does_not_drop_any_positioned_glyphs() {
        let scene = text_scene("A\u{10ffff}B");

        assert_eq!(scene.encoding().resources.glyphs.len(), 3);
    }

    #[cfg(feature = "render")]
    #[kithara::test]
    fn knob_uses_the_expected_commands_for_the_scene() {
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

        let knob = Knob::new(Some("GAIN"), 0.25, builtin::skin());
        let mut recorder = RecordPainter::default();
        knob.paint(
            &mut recorder,
            &mut TextContext::new().unwrap(),
            DIAL,
            CAPTION,
        );
        let commands = recorder.finish();
        let geometry_commands = commands
            .iter()
            .filter(|command| {
                matches!(
                    command,
                    Cmd::FillCircle { .. }
                        | Cmd::StrokeCircle { .. }
                        | Cmd::StrokeArc { .. }
                        | Cmd::StrokeLine { .. }
                )
            })
            .fold(0_u32, |count, _| count + 1);

        let mut scene = Scene::new();
        knob.paint(
            &mut ScenePainter::new(&mut scene),
            &mut TextContext::new().unwrap(),
            DIAL,
            CAPTION,
        );

        assert_eq!(scene.encoding().n_paths, geometry_commands);
    }

    fn text_scene(content: &str) -> Scene {
        let mut scene = Scene::new();
        paint_text(&mut scene, content);
        scene
    }

    fn paint_text(scene: &mut Scene, content: &str) {
        let run = TextContext::new().unwrap().shape(
            content,
            FontFamily::Sans,
            FontWeight::Normal,
            12.0,
            0.0,
            Some(FIXTURE.bounds.w),
        );
        ScenePainter::new(scene).text(
            &run,
            content,
            Transform::translate(Pt {
                x: FIXTURE.bounds.x + (FIXTURE.bounds.w - run.width()) / 2.0,
                y: FIXTURE.bounds.y,
            }),
            FIXTURE.color,
        );
    }

    #[derive(Clone, Copy)]
    struct PaintFixture {
        bounds: Rect,
        color: Rgba,
        point: Pt,
    }

    const FIXTURE: PaintFixture = {
        let color = Rgba {
            a: 1.0,
            b: 0.25,
            g: 0.5,
            r: 0.75,
        };
        PaintFixture {
            bounds: Rect {
                h: 12.0,
                w: 40.0,
                x: 0.0,
                y: 0.0,
            },
            color,
            point: Pt { x: 4.0, y: 4.0 },
        }
    };
}
