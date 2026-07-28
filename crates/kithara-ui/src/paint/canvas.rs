use iced::{
    Color, Font, Point, Radians, Vector,
    alignment::Vertical,
    font::{Family, Stretch, Style, Weight},
    widget::{
        canvas::{Frame, Path, Stroke, Text as CanvasText, path::Arc},
        text::{Alignment, Shaping},
    },
};
use num_traits::cast::AsPrimitive;

use super::{Painter, Pt, Rect, Rgba, TextStyle};
use crate::{
    fonts::catalog,
    skin::{FontFamily, FontWeight},
};

pub(crate) struct IcedPainter<'frame> {
    frame: &'frame mut Frame,
}

impl<'frame> IcedPainter<'frame> {
    pub(crate) const fn new(frame: &'frame mut Frame) -> Self {
        Self { frame }
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

    fn text(&mut self, bounds: Rect, content: &str, style: TextStyle) {
        const HALF: f32 = 0.5;

        let font = font(style.family, style.weight);
        let tracking = style.tracking * style.size;
        let gap_count: f32 = content.chars().count().saturating_sub(1).as_();
        let tracking_width = tracking * gap_count;
        let mut offset = -tracking_width * HALF;
        CanvasText {
            content: content.to_owned(),
            position: Point::new(bounds.x + bounds.w * HALF, bounds.y),
            max_width: bounds.w,
            color: style.color.into(),
            size: style.size.into(),
            font,
            align_x: Alignment::Center,
            align_y: Vertical::Top,
            shaping: Shaping::Advanced,
            ..CanvasText::default()
        }
        .draw_with(|path, color| {
            self.frame.with_save(|frame| {
                frame.translate(Vector::new(offset, 0.0));
                frame.fill(&path, color);
            });
            offset += tracking;
        });
    }
}

pub(crate) const fn font(family: FontFamily, weight: FontWeight) -> Font {
    let face = catalog::select(family, weight);
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
