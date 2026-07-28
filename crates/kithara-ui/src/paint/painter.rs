use crate::skin::{FontFamily, FontWeight};

/// A toolkit-neutral RGBA colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba {
    pub a: f32,
    pub b: f32,
    pub g: f32,
    pub r: f32,
}

/// A toolkit-neutral point in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pt {
    pub x: f32,
    pub y: f32,
}

/// A toolkit-neutral rectangle in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub h: f32,
    pub w: f32,
    pub x: f32,
    pub y: f32,
}

/// Toolkit-neutral text presentation shaped by the painter backend.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct TextStyle {
    pub family: FontFamily,
    pub weight: FontWeight,
    pub color: Rgba,
    pub size: f32,
    /// Additional space between glyphs, relative to the font size.
    pub tracking: f32,
}

impl TextStyle {
    /// Creates toolkit-neutral text presentation.
    #[must_use]
    pub const fn new(
        family: FontFamily,
        weight: FontWeight,
        color: Rgba,
        size: f32,
        tracking: f32,
    ) -> Self {
        Self {
            family,
            weight,
            color,
            size,
            tracking,
        }
    }
}

/// Minimal drawing port implemented by rendering backends.
pub trait Painter {
    fn fill_circle(&mut self, center: Pt, radius: f32, color: Rgba);

    /// Strokes an arc whose angles are expressed in radians.
    fn stroke_arc(
        &mut self,
        center: Pt,
        radius: f32,
        start: f32,
        end: f32,
        color: Rgba,
        width: f32,
    );

    fn stroke_circle(&mut self, center: Pt, radius: f32, color: Rgba, width: f32);

    fn stroke_line(&mut self, from: Pt, to: Pt, color: Rgba, width: f32);

    fn text(&mut self, bounds: Rect, content: &str, style: TextStyle);
}
