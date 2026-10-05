use bon::Builder;

#[cfg(any(feature = "iced", feature = "masonry"))]
use crate::{
    expand::Binding,
    ids::InternId,
    module::TextAlign,
    skin::{ColorRole, FontFamily, FontWeight},
};
use crate::{
    module::TextStyle,
    size::{Dim, SizeSpec},
    skin::SkinDoc,
};

/// A run of text the document supplies or reads.
#[derive(Builder, kithara_derive::Control)]
#[control(size = size(self.style, skin))]
pub(crate) struct Text<#[cfg(any(feature = "iced", feature = "masonry"))] 'a> {
    #[cfg(any(feature = "iced", feature = "masonry"))]
    pub(crate) active: Option<&'a Binding>,
    #[cfg(any(feature = "iced", feature = "masonry"))]
    pub(crate) active_color: Option<ColorRole>,
    #[cfg(any(feature = "iced", feature = "masonry"))]
    pub(crate) color: Option<ColorRole>,
    #[cfg(any(feature = "iced", feature = "masonry"))]
    pub(crate) font: Option<FontFamily>,
    #[cfg(any(feature = "iced", feature = "masonry"))]
    pub(crate) label: Option<InternId>,
    #[cfg(any(feature = "iced", feature = "masonry"))]
    pub(crate) weight: Option<FontWeight>,
    #[cfg(any(feature = "iced", feature = "masonry"))]
    pub(crate) align: TextAlign,
    pub(crate) style: TextStyle,
}

fn size(style: TextStyle, skin: &SkinDoc) -> SizeSpec {
    match style {
        TextStyle::VisFooter => SizeSpec::new(Dim::Fill, Dim::Fixed(skin.vis.footer_height)),
        TextStyle::VisMeta | TextStyle::VisTitle => {
            SizeSpec::new(Dim::Fill, Dim::Fixed(skin.vis.header_height))
        }
        TextStyle::BrandSmall
        | TextStyle::Caption
        | TextStyle::Mono
        | TextStyle::PivotArrow
        | TextStyle::PivotDuration
        | TextStyle::PivotFooter
        | TextStyle::PivotLabel
        | TextStyle::PivotRatio
        | TextStyle::PivotSmall
        | TextStyle::PivotTitle
        | TextStyle::PivotTrackArtist
        | TextStyle::PivotTrackTitle
        | TextStyle::PivotValue => SizeSpec::new(Dim::Shrink, Dim::Shrink),
        TextStyle::Body
        | TextStyle::Brand
        | TextStyle::DeckLetter
        | TextStyle::TrackTitle
        | TextStyle::Telemetry
        | TextStyle::MicroLabel
        | TextStyle::Section => skin.text.size,
    }
}
