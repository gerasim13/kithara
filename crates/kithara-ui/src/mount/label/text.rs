use crate::{
    module::TextStyle,
    mount::Control,
    size::{Dim, SizeSpec},
    skin::SkinDoc,
};

/// A run of text the document supplies or reads.
pub(crate) struct Text {
    pub(crate) style: TextStyle,
}

impl Control for Text {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        match self.style {
            TextStyle::VisFooter => SizeSpec::new(Dim::Fill, Dim::Fixed(skin.vis.footer_height)),
            TextStyle::VisMeta | TextStyle::VisTitle => {
                SizeSpec::new(Dim::Fill, Dim::Fixed(skin.vis.header_height))
            }
            TextStyle::BrandSmall | TextStyle::Mono | TextStyle::Caption => {
                SizeSpec::new(Dim::Shrink, Dim::Fill)
            }
            TextStyle::Body
            | TextStyle::Brand
            | TextStyle::DeckLetter
            | TextStyle::TrackTitle
            | TextStyle::Telemetry
            | TextStyle::MicroLabel
            | TextStyle::Section => skin.text.size,
        }
    }
}
