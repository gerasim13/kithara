use crate::skin::{FontFamily, FontWeight};

/// A toolkit-neutral RGBA colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Rgba {
    pub(crate) a: f32,
    pub(crate) b: f32,
    pub(crate) g: f32,
    pub(crate) r: f32,
}

/// A toolkit-neutral point in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Pt {
    pub(crate) x: f32,
    pub(crate) y: f32,
}

/// A toolkit-neutral rectangle in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Rect {
    pub(crate) h: f32,
    pub(crate) w: f32,
    pub(crate) x: f32,
    pub(crate) y: f32,
}

/// Toolkit-neutral text presentation shaped by the painter backend.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TextStyle {
    pub(crate) family: FontFamily,
    pub(crate) weight: FontWeight,
    pub(crate) color: Rgba,
    pub(crate) size: f32,
    /// Additional space between glyphs, relative to the font size.
    pub(crate) tracking: f32,
}

/// Minimal drawing port implemented by rendering backends.
pub(crate) trait Painter {
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
