use kithara_platform::sync::{Arc as SharedArc, OnceLock};
use skrifa::{
    FontRef,
    instance::{LocationRef, Size},
    prelude::MetadataProvider,
};
use vello::{
    Glyph, Scene,
    kurbo::{Affine, Arc, Cap, Circle, Join, Line, Point, Stroke, Vec2},
    peniko::{Blob, Color, Fill, FontData},
};

use super::{Painter, Pt, Rect, Rgba, TextStyle};
use crate::fonts::catalog::{FontFace, select};

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

    fn text(&mut self, bounds: Rect, content: &str, style: TextStyle) {
        let face = select(style.family, style.weight);
        let font = cached_font(face);
        let Some(font_ref) = font.reference.as_ref() else {
            return;
        };
        let glyphs = shape(font_ref, face, content, style.size, style.tracking);
        let width = glyphs.iter().map(|glyph| glyph.advance).sum::<f32>();
        let metrics = font_ref.metrics(Size::new(style.size), LocationRef::default());
        let baseline = bounds.y + metrics.ascent;
        let mut x = bounds.x + (bounds.w - width) * 0.5;
        let positioned = glyphs.into_iter().map(|glyph| {
            let positioned = Glyph {
                id: glyph.id,
                x,
                y: baseline,
            };
            x += glyph.advance;
            positioned
        });
        self.scene
            .draw_glyphs(&font.data)
            .font_size(style.size)
            .brush(Color::from(style.color))
            .draw(Fill::NonZero, positioned);
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

struct ShapedGlyph {
    advance: f32,
    id: u32,
}

fn shape(
    font: &FontRef<'_>,
    face: FontFace,
    content: &str,
    size: f32,
    tracking: f32,
) -> Vec<ShapedGlyph> {
    let charmap = font.charmap();
    let metrics = font.glyph_metrics(Size::new(size), LocationRef::default());
    let tracking = tracking * size;
    let mut glyphs = Vec::with_capacity(content.chars().count());
    for character in content.chars() {
        let Some(id) = charmap.map(character) else {
            tracing::debug!(?face, ?character, "font has no mapped glyph");
            continue;
        };
        let Some(advance) = metrics.advance_width(id) else {
            tracing::debug!(
                ?face,
                glyph_id = id.to_u32(),
                ?character,
                "font has no glyph advance"
            );
            continue;
        };
        glyphs.push(ShapedGlyph {
            advance,
            id: id.to_u32(),
        });
    }
    if let Some((_, rest)) = glyphs.split_last_mut() {
        for glyph in rest {
            glyph.advance += tracking;
        }
    }
    glyphs
}

struct CachedFont {
    data: FontData,
    reference: Option<FontRef<'static>>,
}

fn cached_font(face: FontFace) -> &'static CachedFont {
    static INTER_REGULAR: OnceLock<CachedFont> = OnceLock::new();
    static INTER_SEMIBOLD: OnceLock<CachedFont> = OnceLock::new();
    static JETBRAINS_MONO_REGULAR: OnceLock<CachedFont> = OnceLock::new();
    static JETBRAINS_MONO_MEDIUM: OnceLock<CachedFont> = OnceLock::new();
    static JETBRAINS_MONO_SEMIBOLD: OnceLock<CachedFont> = OnceLock::new();
    static SPACE_GROTESK_REGULAR: OnceLock<CachedFont> = OnceLock::new();
    static SPACE_GROTESK_MEDIUM: OnceLock<CachedFont> = OnceLock::new();
    static SPACE_GROTESK_SEMIBOLD: OnceLock<CachedFont> = OnceLock::new();
    static SPACE_GROTESK_BOLD: OnceLock<CachedFont> = OnceLock::new();

    let cache = match face {
        FontFace::InterRegular => &INTER_REGULAR,
        FontFace::InterSemibold => &INTER_SEMIBOLD,
        FontFace::JetBrainsMonoRegular => &JETBRAINS_MONO_REGULAR,
        FontFace::JetBrainsMonoMedium => &JETBRAINS_MONO_MEDIUM,
        FontFace::JetBrainsMonoSemibold => &JETBRAINS_MONO_SEMIBOLD,
        FontFace::SpaceGroteskRegular => &SPACE_GROTESK_REGULAR,
        FontFace::SpaceGroteskMedium => &SPACE_GROTESK_MEDIUM,
        FontFace::SpaceGroteskSemibold => &SPACE_GROTESK_SEMIBOLD,
        FontFace::SpaceGroteskBold => &SPACE_GROTESK_BOLD,
    };
    cache.get_or_init(|| {
        let bytes = face.bytes();
        let reference = match FontRef::new(bytes) {
            Ok(reference) => Some(reference),
            Err(error) => {
                tracing::error!(
                    ?face,
                    ?error,
                    bytes = bytes.len(),
                    "embedded font data is invalid"
                );
                None
            }
        };
        CachedFont {
            data: FontData::new(Blob::new(SharedArc::new(bytes)), 0),
            reference,
        }
    })
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;
    use crate::skin::{FontFamily, FontWeight};
    #[cfg(feature = "render")]
    use crate::{
        atoms::knob::Knob,
        builtin,
        paint::record::{Cmd, RecordPainter},
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
        ScenePainter::new(&mut scene).text(FIXTURE.bounds, "GAIN", FIXTURE.text_style);
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
    fn missing_glyph_does_not_drop_the_rest_of_the_run() {
        let scene = text_scene("A\u{10ffff}B");

        assert_eq!(scene.encoding().resources.glyphs.len(), 2);
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
        knob.paint(&mut recorder, DIAL, CAPTION);
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
        knob.paint(&mut ScenePainter::new(&mut scene), DIAL, CAPTION);

        assert_eq!(scene.encoding().n_paths, geometry_commands);
    }

    fn text_scene(content: &str) -> Scene {
        let mut scene = Scene::new();
        ScenePainter::new(&mut scene).text(FIXTURE.bounds, content, FIXTURE.text_style);
        scene
    }

    #[derive(Clone, Copy)]
    struct PaintFixture {
        bounds: Rect,
        color: Rgba,
        point: Pt,
        text_style: TextStyle,
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
            text_style: TextStyle::new(FontFamily::Sans, FontWeight::Normal, color, 12.0, 0.0),
        }
    };
}
