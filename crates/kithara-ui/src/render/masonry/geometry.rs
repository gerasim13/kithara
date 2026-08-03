use crate::{
    skin::TextRoleSkin,
    text::{GlyphRun, TextContext},
};

/// Logical two-dimensional extent used by custom Masonry content.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct Size2 {
    pub w: f32,
    pub h: f32,
}

impl Size2 {
    #[must_use]
    pub const fn new(w: f32, h: f32) -> Self {
        Self { w, h }
    }
}

/// Minimum and maximum logical extents available during intrinsic measurement.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct SizeLimits {
    min: Size2,
    max: Size2,
}

impl SizeLimits {
    #[must_use]
    pub const fn new(min: Size2, max: Size2) -> Self {
        Self { min, max }
    }

    #[must_use]
    pub const fn min(self) -> Size2 {
        self.min
    }

    #[must_use]
    pub const fn max(self) -> Size2 {
        self.max
    }
}

/// Borrowed access to Kithara's canonical text shaper.
///
/// This facade owns no cache and no font collection. Custom widgets therefore
/// receive the same metrics as built-in Kithara controls instead of creating a
/// second text answer at the host boundary.
pub struct TextMeasurer<'a> {
    context: &'a mut TextContext,
}

impl<'a> TextMeasurer<'a> {
    pub(crate) const fn new(context: &'a mut TextContext) -> Self {
        Self { context }
    }

    /// Shapes text with the complete skin role and an optional wrapping width.
    #[must_use]
    pub fn shape(&mut self, content: &str, role: TextRoleSkin, max_width: Option<f32>) -> GlyphRun {
        self.context.shape(content, role, max_width)
    }

    /// Measures a shaped run without retaining a second cached layout.
    #[must_use]
    pub fn measure(&mut self, content: &str, role: TextRoleSkin, max_width: Option<f32>) -> Size2 {
        let run = self.shape(content, role, max_width);
        Size2::new(run.width(), run.height())
    }
}
