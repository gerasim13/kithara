use super::FontId;

/// A positioned glyph in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Glyph {
    pub id: u32,
    pub x: f32,
    pub y: f32,
}

/// A measured, positioned glyph run using one embedded font face and size.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphRun {
    font: FontId,
    glyphs: Vec<Glyph>,
    height: f32,
    size: f32,
    width: f32,
}

impl GlyphRun {
    pub(super) fn new(
        font: FontId,
        glyphs: Vec<Glyph>,
        height: f32,
        size: f32,
        width: f32,
    ) -> Self {
        Self {
            font,
            glyphs,
            height,
            size,
            width,
        }
    }

    /// Returns the embedded face used by every glyph in the run.
    #[must_use]
    pub const fn font(&self) -> FontId {
        self.font
    }

    /// Returns the positioned glyphs in visual order.
    #[must_use]
    pub fn glyphs(&self) -> &[Glyph] {
        &self.glyphs
    }

    /// Returns the measured layout height in logical pixels.
    #[must_use]
    pub const fn height(&self) -> f32 {
        self.height
    }

    /// Returns the font size in logical pixels.
    #[must_use]
    pub const fn size(&self) -> f32 {
        self.size
    }

    /// Returns the measured layout width in logical pixels.
    #[must_use]
    pub const fn width(&self) -> f32 {
        self.width
    }
}
