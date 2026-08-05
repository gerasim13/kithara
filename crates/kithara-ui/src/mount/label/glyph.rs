use bon::Builder;

use crate::{
    expand::Binding,
    module::{GlyphStyle, IconName},
    mount::Control,
    size::{Dim, SizeSpec},
    skin::{ColorRole, SkinDoc},
};

/// A single icon, drawn as a text glyph.
#[derive(Builder)]
pub(crate) struct Glyph<'a> {
    pub(crate) active: Option<&'a Binding>,
    pub(crate) active_color: Option<ColorRole>,
    pub(crate) active_icon: Option<IconName>,
    pub(crate) color: Option<ColorRole>,
    pub(crate) icon: IconName,
    pub(crate) style: GlyphStyle,
}

impl Control for Glyph<'_> {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        match self.style {
            GlyphStyle::Menu => cell(skin.menu.icon_size),
            GlyphStyle::MenuBurger => cell(skin.menu.burger_icon_size),
            GlyphStyle::MenuSmall => cell(skin.menu.small_icon_size),
            GlyphStyle::MenuCell => cell(skin.menu.cell_icon_size),
            GlyphStyle::Default | GlyphStyle::Vis => SizeSpec::new(
                Dim::Fixed(skin.nav.header_icon_size),
                Dim::Fixed(skin.nav.header_height),
            ),
        }
    }
}

/// An icon renders as a text glyph, whose line box is taller than the icon
/// size; the row it sits in owns the height so the glyph centres against its
/// siblings.
fn cell(side: f32) -> SizeSpec {
    SizeSpec::new(Dim::Fixed(side), Dim::Fill)
}
