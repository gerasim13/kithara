use kithara_platform::sync::Arc as SharedArc;
use vello::{
    Glyph, Scene,
    kurbo::{
        Affine, Arc, Cap, Circle, Join, Line, Point, Rect as KurboRect, RoundedRect, Shape, Stroke,
        Vec2,
    },
    peniko::{Blob, Color, Fill, FontData},
};

use crate::{
    draw::{Backend, Geom, Pt, Rgba, Transform},
    text::GlyphRun,
};

/// A backend that encodes drawing commands into a Vello [`Scene`].
pub struct VelloBackend<'scene> {
    scene: &'scene mut Scene,
}

impl<'scene> VelloBackend<'scene> {
    /// Creates a backend for `scene`.
    pub const fn new(scene: &'scene mut Scene) -> Self {
        Self { scene }
    }

    fn fill_shape(&mut self, shape: &impl Shape, color: Rgba) {
        self.scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            Color::from(color),
            None,
            shape,
        );
    }

    fn stroke_shape(&mut self, shape: &impl Shape, color: Rgba, width: f32) {
        self.scene.stroke(
            &stroke(width),
            Affine::IDENTITY,
            Color::from(color),
            None,
            shape,
        );
    }
}

impl Backend for VelloBackend<'_> {
    fn fill(&mut self, geom: Geom, color: Rgba) {
        match geom {
            Geom::Arc {
                center,
                radius,
                start,
                end,
            } => self.fill_shape(
                &Arc::new(
                    center,
                    Vec2::splat(f64::from(radius)),
                    f64::from(start),
                    f64::from(end - start),
                    0.0,
                ),
                color,
            ),
            Geom::Circle { center, radius } => {
                self.fill_shape(&Circle::new(center, f64::from(radius)), color);
            }
            Geom::Line { from, to } => self.fill_shape(&Line::new(from, to), color),
            Geom::Rect(rect) => self.fill_shape(
                &KurboRect::new(
                    f64::from(rect.x),
                    f64::from(rect.y),
                    f64::from(rect.x + rect.w),
                    f64::from(rect.y + rect.h),
                ),
                color,
            ),
            Geom::RoundedRect { rect, radius } => self.fill_shape(
                &RoundedRect::new(
                    f64::from(rect.x),
                    f64::from(rect.y),
                    f64::from(rect.x + rect.w),
                    f64::from(rect.y + rect.h),
                    f64::from(radius),
                ),
                color,
            ),
        }
    }

    fn stroke(&mut self, geom: Geom, color: Rgba, width: f32) {
        match geom {
            Geom::Arc {
                center,
                radius,
                start,
                end,
            } => self.stroke_shape(
                &Arc::new(
                    center,
                    Vec2::splat(f64::from(radius)),
                    f64::from(start),
                    f64::from(end - start),
                    0.0,
                ),
                color,
                width,
            ),
            Geom::Circle { center, radius } => {
                self.stroke_shape(&Circle::new(center, f64::from(radius)), color, width);
            }
            Geom::Line { from, to } => self.stroke_shape(&Line::new(from, to), color, width),
            Geom::Rect(rect) => self.stroke_shape(
                &KurboRect::new(
                    f64::from(rect.x),
                    f64::from(rect.y),
                    f64::from(rect.x + rect.w),
                    f64::from(rect.y + rect.h),
                ),
                color,
                width,
            ),
            Geom::RoundedRect { rect, radius } => self.stroke_shape(
                &RoundedRect::new(
                    f64::from(rect.x),
                    f64::from(rect.y),
                    f64::from(rect.x + rect.w),
                    f64::from(rect.y + rect.h),
                    f64::from(radius),
                ),
                color,
                width,
            ),
        }
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
    use crate::{
        draw::{DrawCmd, DrawListBuilder, Rect, replay},
        skin::{ColorRole, FontFamily, FontWeight, TextRoleSkin},
        text::TextContext,
    };

    #[kithara::test]
    fn every_draw_operation_adds_to_the_encoding() {
        let run = TextContext::new()
            .unwrap()
            .shape("GAIN", FIXTURE.role, Some(FIXTURE.bounds.w));
        let mut builder = DrawListBuilder::default();
        builder.fill_circle(FIXTURE.point, 5.0, FIXTURE.color);
        builder.stroke_arc(FIXTURE.point, 5.0, 0.0, 1.0, FIXTURE.color, 1.0);
        builder.stroke_circle(FIXTURE.point, 5.0, FIXTURE.color, 1.0);
        builder.stroke_line(FIXTURE.point, Pt { x: 8.0, y: 8.0 }, FIXTURE.color, 1.0);
        builder.fill_rect(FIXTURE.bounds, FIXTURE.color);
        builder.fill_rounded_rect(FIXTURE.bounds, 3.0, FIXTURE.color);
        builder.stroke_rounded_rect(FIXTURE.bounds, 3.0, FIXTURE.color, 1.0);
        builder.text(&run, "GAIN", Transform::IDENTITY, FIXTURE.color);
        let list = builder.finish();
        let mut scene = Scene::new();

        replay(&list, &mut VelloBackend::new(&mut scene));

        assert_eq!(scene.encoding().n_paths, 7);
        assert!(!scene.encoding().resources.glyphs.is_empty());
    }

    #[kithara::test]
    fn stroke_width_changes_the_encoding() {
        let thin = line_scene(1.0);
        let thick = line_scene(3.0);

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

    fn line_scene(width: f32) -> Scene {
        let mut builder = DrawListBuilder::default();
        builder.stroke_line(FIXTURE.point, Pt { x: 8.0, y: 8.0 }, FIXTURE.color, width);
        let mut scene = Scene::new();
        replay(&builder.finish(), &mut VelloBackend::new(&mut scene));
        scene
    }

    fn text_scene(content: &str) -> Scene {
        let run = TextContext::new()
            .unwrap()
            .shape(content, FIXTURE.role, Some(FIXTURE.bounds.w));
        let mut builder = DrawListBuilder::default();
        builder.text(
            &run,
            content,
            Transform::translate(Pt {
                x: FIXTURE.bounds.x + (FIXTURE.bounds.w - run.width()) / 2.0,
                y: FIXTURE.bounds.y,
            }),
            FIXTURE.color,
        );
        let mut scene = Scene::new();
        replay(&builder.finish(), &mut VelloBackend::new(&mut scene));
        scene
    }

    #[derive(Clone, Copy)]
    struct DrawFixture {
        bounds: Rect,
        color: Rgba,
        point: Pt,
        role: TextRoleSkin,
    }

    const FIXTURE: DrawFixture = {
        let color = Rgba {
            a: 1.0,
            b: 0.25,
            g: 0.5,
            r: 0.75,
        };
        DrawFixture {
            bounds: Rect {
                h: 12.0,
                w: 40.0,
                x: 0.0,
                y: 0.0,
            },
            color,
            point: Pt { x: 4.0, y: 4.0 },
            role: TextRoleSkin {
                color: ColorRole::Text,
                font: FontFamily::Sans,
                size: 12.0,
                spacing: 0.0,
                weight: FontWeight::Normal,
            },
        }
    };

    #[cfg(feature = "render")]
    #[kithara::test]
    fn replaying_a_knob_list_encodes_one_path_per_geometry_command() {
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

        let mut builder = DrawListBuilder::default();
        crate::atoms::knob::Knob::new(Some("GAIN"), 0.25, crate::builtin::skin()).paint(
            &mut builder,
            &mut TextContext::new().unwrap(),
            DIAL,
            CAPTION,
        );
        let list = builder.finish();
        let geometry = list
            .commands()
            .iter()
            .filter(|command| !matches!(command, DrawCmd::Text { .. }))
            .fold(0_u32, |count, _| count + 1);

        let mut scene = Scene::new();
        replay(&list, &mut VelloBackend::new(&mut scene));

        assert_eq!(scene.encoding().n_paths, geometry);
        assert!(!scene.encoding().resources.glyphs.is_empty());
    }
}
