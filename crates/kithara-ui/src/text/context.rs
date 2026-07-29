use std::borrow::Cow;

use parley::{
    FontContext, LayoutContext, PositionedLayoutItem, StyleProperty,
    fontique::SourceCache,
    style::{FontFamily as ParleyFamily, FontWeight as ParleyWeight},
};

use super::{Glyph, GlyphRun, TextError, TextResources, select};
use crate::skin::{FontFamily, FontWeight};

/// Owns the embedded font collection and Parley shaping scratch space.
pub struct TextContext {
    fonts: FontContext,
    layout: LayoutContext<()>,
    #[cfg(feature = "render")]
    resources: TextResources,
}

impl TextContext {
    /// Creates a text context containing only `kithara-ui`'s embedded faces.
    ///
    /// # Errors
    ///
    /// Returns [`TextError`] when a compile-time embedded face is invalid.
    pub fn new() -> Result<Self, TextError> {
        Ok(TextResources::new()?.into())
    }

    #[cfg(test)]
    fn family_names(&mut self) -> Vec<String> {
        self.fonts
            .collection
            .family_names()
            .map(ToOwned::to_owned)
            .collect()
    }
}

impl From<TextResources> for TextContext {
    fn from(resources: TextResources) -> Self {
        Self {
            fonts: FontContext {
                collection: resources.collection(),
                source_cache: SourceCache::default(),
            },
            layout: LayoutContext::new(),
            #[cfg(feature = "render")]
            resources,
        }
    }
}

impl TextContext {
    #[cfg(feature = "render")]
    pub(crate) fn resources(&self) -> &TextResources {
        &self.resources
    }

    /// Shapes and measures text with the selected embedded face.
    ///
    /// `tracking` is additional letter spacing relative to `size`. `max_width`
    /// is `None` for an unbounded line or `Some(width)` for line breaking.
    #[must_use]
    pub fn shape(
        &mut self,
        content: &str,
        family: FontFamily,
        weight: FontWeight,
        size: f32,
        tracking: f32,
        max_width: Option<f32>,
    ) -> GlyphRun {
        let font = select(family, weight);
        let mut builder = self
            .layout
            .ranged_builder(&mut self.fonts, content, 1.0, false);
        builder.push_default(ParleyFamily::Named(Cow::Borrowed(font.family_name())));
        builder.push_default(StyleProperty::FontWeight(parley_weight(weight)));
        builder.push_default(StyleProperty::FontSize(size));
        builder.push_default(StyleProperty::LetterSpacing(tracking * size));
        let mut layout = builder.build(content);
        layout.break_all_lines(max_width);

        let mut glyphs = Vec::new();
        for line in layout.lines() {
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(run) = item else {
                    continue;
                };
                glyphs.extend(run.positioned_glyphs().map(|glyph| Glyph {
                    id: glyph.id,
                    x: glyph.x,
                    y: glyph.y,
                }));
            }
        }
        GlyphRun::new(
            font,
            glyphs,
            layout.height(),
            #[cfg(feature = "render")]
            self.resources.outline_font(font),
            size,
            layout.width(),
        )
    }
}

const fn parley_weight(weight: FontWeight) -> ParleyWeight {
    match weight {
        FontWeight::Normal => ParleyWeight::NORMAL,
        FontWeight::Medium => ParleyWeight::MEDIUM,
        FontWeight::Semibold => ParleyWeight::SEMI_BOLD,
        FontWeight::Bold => ParleyWeight::BOLD,
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;
    use crate::text::FontId;

    #[kithara::test]
    fn context_registers_only_embedded_families() {
        let mut context = TextContext::new().unwrap();
        let mut families = context.family_names();
        families.sort();

        assert_eq!(
            families,
            ["Inter", "JetBrains Mono", "Space Grotesk"],
            "the collection is embedded-only until system fallback is owned"
        );
    }

    #[kithara::test]
    fn shape_returns_positioned_glyphs_and_measurement() {
        let run = TextContext::new().unwrap().shape(
            "GAIN",
            FontFamily::Sans,
            FontWeight::Semibold,
            12.0,
            0.0,
            None,
        );

        assert_eq!(run.font(), FontId::InterSemibold);
        assert!(!run.glyphs().is_empty());
        assert!(
            run.glyphs()
                .iter()
                .all(|glyph| glyph.x.is_finite() && glyph.y.is_finite())
        );
        assert!(run.width() > 0.0);
        assert!(run.height() > 0.0);
    }

    #[kithara::test]
    fn tracking_increases_measured_width() {
        let mut context = TextContext::new().unwrap();
        let plain = context.shape(
            "GAIN",
            FontFamily::Sans,
            FontWeight::Normal,
            12.0,
            0.0,
            None,
        );
        let tracked = context.shape(
            "GAIN",
            FontFamily::Sans,
            FontWeight::Normal,
            12.0,
            0.1,
            None,
        );

        assert!(tracked.width() > plain.width());
    }

    #[kithara::test]
    fn max_width_breaks_lines_and_changes_measurement() {
        let mut context = TextContext::new().unwrap();
        let unbounded = context.shape(
            "GAIN GAIN GAIN",
            FontFamily::Sans,
            FontWeight::Normal,
            12.0,
            0.0,
            None,
        );
        let wrapped = context.shape(
            "GAIN GAIN GAIN",
            FontFamily::Sans,
            FontWeight::Normal,
            12.0,
            0.0,
            Some(35.0),
        );

        assert!(wrapped.width() <= 35.0);
        assert!(wrapped.height() > unbounded.height());
    }
}
