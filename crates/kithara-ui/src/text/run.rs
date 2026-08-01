use super::FontId;

/// A positioned glyph in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Glyph {
    pub id: u32,
    pub x: f32,
    pub y: f32,
}

/// A stretch of a shaped run drawn with one embedded face.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphSegment {
    face: FontId,
    glyphs: Vec<Glyph>,
}

impl GlyphSegment {
    pub(super) const fn new(face: FontId, glyphs: Vec<Glyph>) -> Self {
        Self { face, glyphs }
    }

    /// Returns the embedded face every glyph in this segment belongs to.
    #[must_use]
    pub const fn face(&self) -> FontId {
        self.face
    }

    /// Returns the positioned glyphs in visual order.
    #[must_use]
    pub fn glyphs(&self) -> &[Glyph] {
        &self.glyphs
    }
}

/// A measured, positioned run of text at one size, segmented by face.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphRun {
    segments: Vec<GlyphSegment>,
    height: f32,
    size: f32,
    width: f32,
}

impl GlyphRun {
    pub(super) const fn new(
        segments: Vec<GlyphSegment>,
        height: f32,
        size: f32,
        width: f32,
    ) -> Self {
        Self {
            segments,
            height,
            size,
            width,
        }
    }

    /// Returns the single-face segments in visual order.
    #[must_use]
    pub fn segments(&self) -> &[GlyphSegment] {
        &self.segments
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
