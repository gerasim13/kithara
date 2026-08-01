use std::borrow::Cow;

use parley::{
    FontContext, LayoutContext, PositionedLayoutItem, StyleProperty,
    fontique::SourceCache,
    style::{FontFamily as ParleyFamily, FontWeight as ParleyWeight},
};

use super::{FontId, Glyph, GlyphRun, TextError, TextResources, select};
use crate::skin::{FontWeight, TextRoleSkin};

/// Owns the embedded font collection and Parley shaping scratch space.
pub struct TextContext {
    fonts: FontContext,
    layout: LayoutContext<()>,
}

#[derive(Clone, Copy)]
struct FaceStyle {
    font: FontId,
    size: f32,
    spacing: f32,
    weight: FontWeight,
}

impl TextContext {
    /// Creates a text context containing only `kithara-ui`'s embedded faces.
    ///
    /// # Errors
    ///
    /// Returns [`TextError`] when a compile-time embedded face is invalid.
    pub fn new() -> Result<Self, TextError> {
        Ok(Self::from(&TextResources::new()?))
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

impl From<&TextResources> for TextContext {
    fn from(resources: &TextResources) -> Self {
        Self {
            fonts: FontContext {
                collection: resources.collection(),
                source_cache: SourceCache::default(),
            },
            layout: LayoutContext::new(),
        }
    }
}

impl TextContext {
    /// Shapes and measures text in a skin role with the selected embedded face.
    ///
    /// `max_width` is `None` for an unbounded line or `Some(width)` for line
    /// breaking. The role travels whole rather than as a face and a size,
    /// because `spacing` is letter tracking: a signature that took those loose
    /// let a caller shape text and drop the tracking the skin declared, which
    /// is what every string rendered through iced did.
    #[must_use]
    pub fn shape(&mut self, content: &str, role: TextRoleSkin, max_width: Option<f32>) -> GlyphRun {
        self.shape_run(
            content,
            FaceStyle {
                font: select(role.font, role.weight),
                size: role.size,
                spacing: role.spacing,
                weight: role.weight,
            },
            max_width,
        )
    }

    #[cfg(feature = "render")]
    pub(crate) fn shape_lucide(&mut self, content: &str, size: f32) -> GlyphRun {
        self.shape_run(
            content,
            FaceStyle {
                font: FontId::Lucide,
                size,
                spacing: 0.0,
                weight: FontWeight::Normal,
            },
            None,
        )
    }

    fn shape_run(&mut self, content: &str, style: FaceStyle, max_width: Option<f32>) -> GlyphRun {
        let mut builder = self
            .layout
            .ranged_builder(&mut self.fonts, content, 1.0, false);
        builder.push_default(ParleyFamily::Named(Cow::Borrowed(style.font.family_name())));
        builder.push_default(StyleProperty::FontWeight(parley_weight(style.weight)));
        builder.push_default(StyleProperty::FontSize(style.size));
        builder.push_default(StyleProperty::LetterSpacing(style.spacing * style.size));
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
            style.font,
            glyphs,
            layout.height(),
            style.size,
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
    use crate::skin::{ColorRole, FontFamily};

    fn role(family: FontFamily, weight: FontWeight, size: f32, spacing: f32) -> TextRoleSkin {
        TextRoleSkin {
            color: ColorRole::Text,
            font: family,
            size,
            spacing,
            weight,
        }
    }

    #[kithara::test]
    fn context_registers_only_embedded_families() {
        let mut context = TextContext::new().unwrap();
        let mut families = context.family_names();
        families.sort();

        assert_eq!(
            families,
            ["Inter", "JetBrains Mono", "Space Grotesk", "lucide"],
            "the ten-face embedded catalog is the owned registration contract; system fallback remains excluded"
        );
        assert_eq!(
            FontId::ALL,
            [
                FontId::InterRegular,
                FontId::InterSemibold,
                FontId::JetBrainsMonoRegular,
                FontId::JetBrainsMonoMedium,
                FontId::JetBrainsMonoSemibold,
                FontId::SpaceGroteskRegular,
                FontId::SpaceGroteskMedium,
                FontId::SpaceGroteskSemibold,
                FontId::SpaceGroteskBold,
                FontId::Lucide,
            ],
            "all ten registered embedded faces are named by the catalog contract"
        );
    }

    #[kithara::test]
    fn shape_returns_positioned_glyphs_and_measurement() {
        let run = TextContext::new().unwrap().shape(
            "GAIN",
            role(FontFamily::Sans, FontWeight::Semibold, 12.0, 0.0),
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

    #[cfg(feature = "render")]
    #[kithara::test]
    fn explicit_lucide_face_shapes_an_icon_glyph() {
        let content = char::from(lucide_icons::Icon::Play).to_string();
        let run = TextContext::new().unwrap().shape_lucide(&content, 14.0);

        assert_eq!(run.font(), FontId::Lucide);
        assert_eq!(run.glyphs().len(), 1);
        assert!(run.width() > 0.0);
        assert!(run.height() > 0.0);
    }

    #[kithara::test]
    fn tracking_increases_measured_width() {
        let mut context = TextContext::new().unwrap();
        let plain = context.shape(
            "GAIN",
            role(FontFamily::Sans, FontWeight::Normal, 12.0, 0.0),
            None,
        );
        let tracked = context.shape(
            "GAIN",
            role(FontFamily::Sans, FontWeight::Normal, 12.0, 0.1),
            None,
        );

        assert!(tracked.width() > plain.width());
    }

    #[kithara::test]
    fn max_width_breaks_lines_and_changes_measurement() {
        let mut context = TextContext::new().unwrap();
        let unbounded = context.shape(
            "GAIN GAIN GAIN",
            role(FontFamily::Sans, FontWeight::Normal, 12.0, 0.0),
            None,
        );
        let wrapped = context.shape(
            "GAIN GAIN GAIN",
            role(FontFamily::Sans, FontWeight::Normal, 12.0, 0.0),
            Some(35.0),
        );

        assert!(wrapped.width() <= 35.0);
        assert!(wrapped.height() > unbounded.height());
    }
}
