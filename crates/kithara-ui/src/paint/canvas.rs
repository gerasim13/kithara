use iced::{
    Color, Font, Point, Radians,
    font::{Family, Stretch, Style, Weight},
    widget::canvas::{
        Frame, Path, Stroke,
        path::{Arc, Builder},
    },
};
use skrifa::{
    GlyphId,
    instance::{LocationRef, Size},
    outline::{DrawSettings, OutlinePen},
};

use super::{Painter, Pt, Rgba, Transform};
use crate::{
    skin::{FontFamily, FontWeight},
    text::{GlyphRun, TextResources, select},
};

pub(crate) struct IcedPainter<'frame> {
    frame: &'frame mut Frame,
    resources: &'frame TextResources,
}

impl<'frame> IcedPainter<'frame> {
    pub(crate) const fn new(frame: &'frame mut Frame, resources: &'frame TextResources) -> Self {
        Self { frame, resources }
    }
}

impl Painter for IcedPainter<'_> {
    fn fill_circle(&mut self, center: Pt, radius: f32, color: Rgba) {
        self.frame
            .fill(&Path::circle(center.into(), radius), Color::from(color));
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
        let path = Path::new(|builder| {
            builder.arc(Arc {
                radius,
                center: center.into(),
                start_angle: Radians(start),
                end_angle: Radians(end),
            });
        });
        self.frame.stroke(&path, stroke(color, width));
    }

    fn stroke_circle(&mut self, center: Pt, radius: f32, color: Rgba, width: f32) {
        self.frame
            .stroke(&Path::circle(center.into(), radius), stroke(color, width));
    }

    fn stroke_line(&mut self, from: Pt, to: Pt, color: Rgba, width: f32) {
        self.frame
            .stroke(&Path::line(from.into(), to.into()), stroke(color, width));
    }

    fn text(&mut self, run: &GlyphRun, _content: &str, transform: Transform, color: Rgba) {
        let outlines = self.resources.outlines(run.font());
        let path = Path::new(|builder| {
            for glyph in run.glyphs() {
                let Some(outline) = outlines.get(GlyphId::new(glyph.id)) else {
                    continue;
                };
                let mut pen = IcedOutline {
                    builder,
                    glyph: Pt {
                        x: glyph.x,
                        y: glyph.y,
                    },
                    transform,
                };
                let settings =
                    DrawSettings::unhinted(Size::new(run.size()), LocationRef::default());
                if let Err(error) = outline.draw(settings, &mut pen) {
                    tracing::warn!(
                        face = ?run.font(),
                        glyph_id = glyph.id,
                        ?error,
                        "failed to draw embedded glyph outline"
                    );
                }
            }
        });
        self.frame.fill(&path, Color::from(color));
    }
}

pub(crate) const fn font(family: FontFamily, weight: FontWeight) -> Font {
    let face = select(family, weight);
    let weight = match weight {
        FontWeight::Normal => Weight::Normal,
        FontWeight::Medium => Weight::Medium,
        FontWeight::Semibold => Weight::Semibold,
        FontWeight::Bold => Weight::Bold,
    };
    Font {
        family: Family::Name(face.family_name()),
        weight,
        stretch: Stretch::Normal,
        style: Style::Normal,
    }
}

struct IcedOutline<'builder> {
    builder: &'builder mut Builder,
    glyph: Pt,
    transform: Transform,
}

impl IcedOutline<'_> {
    fn point(&self, x: f32, y: f32) -> Point {
        self.transform
            .apply(Pt {
                x: self.glyph.x + x,
                y: self.glyph.y - y,
            })
            .into()
    }
}

impl OutlinePen for IcedOutline<'_> {
    fn close(&mut self) {
        self.builder.close();
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.builder
            .bezier_curve_to(self.point(cx0, cy0), self.point(cx1, cy1), self.point(x, y));
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.builder.line_to(self.point(x, y));
    }

    fn move_to(&mut self, x: f32, y: f32) {
        self.builder.move_to(self.point(x, y));
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.builder
            .quadratic_curve_to(self.point(cx0, cy0), self.point(x, y));
    }
}

impl From<Rgba> for Color {
    fn from(color: Rgba) -> Self {
        Self {
            a: color.a,
            b: color.b,
            g: color.g,
            r: color.r,
        }
    }
}

impl From<Color> for Rgba {
    fn from(color: Color) -> Self {
        Self {
            a: color.a,
            b: color.b,
            g: color.g,
            r: color.r,
        }
    }
}

impl From<Pt> for Point {
    fn from(point: Pt) -> Self {
        Self::new(point.x, point.y)
    }
}

fn stroke(color: Rgba, width: f32) -> Stroke<'static> {
    Stroke::default()
        .with_color(Color::from(color))
        .with_width(width)
}
