use crate::{
    module::GlyphStyle,
    mount::Control,
    size::{Dim, SizeSpec},
    skin::SkinDoc,
};

/// A single icon, drawn as a text glyph.
pub(crate) struct Glyph {
    pub(crate) style: GlyphStyle,
}

impl Control for Glyph {
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
