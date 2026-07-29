use std::fmt;

#[cfg(feature = "render")]
use skrifa::FontRef;

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
#[derive(Clone)]
pub struct GlyphRun {
    font: FontId,
    glyphs: Vec<Glyph>,
    height: f32,
    #[cfg(feature = "render")]
    outline_font: FontRef<'static>,
    size: f32,
    width: f32,
}

impl GlyphRun {
    pub(super) fn new(
        font: FontId,
        glyphs: Vec<Glyph>,
        height: f32,
        #[cfg(feature = "render")] outline_font: FontRef<'static>,
        size: f32,
        width: f32,
    ) -> Self {
        Self {
            font,
            glyphs,
            height,
            #[cfg(feature = "render")]
            outline_font,
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

    #[cfg(feature = "render")]
    pub(crate) const fn outline_font(&self) -> &FontRef<'static> {
        &self.outline_font
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

impl fmt::Debug for GlyphRun {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GlyphRun")
            .field("font", &self.font)
            .field("glyphs", &self.glyphs)
            .field("height", &self.height)
            .field("size", &self.size)
            .field("width", &self.width)
            .finish_non_exhaustive()
    }
}

impl PartialEq for GlyphRun {
    fn eq(&self, other: &Self) -> bool {
        self.font == other.font
            && self.glyphs == other.glyphs
            && self.height == other.height
            && self.size == other.size
            && self.width == other.width
    }
}
