use bon::Builder;

#[cfg(any(feature = "iced", feature = "masonry"))]
use crate::ids::InternId;
use crate::size::SizeSpec;

/// The window's own caption strip, which also drags the window.
#[derive(Builder, kithara_derive::Control)]
#[control(size = SizeSpec::FILL)]
pub(crate) struct TitleBar {
    #[cfg(any(feature = "iced", feature = "masonry"))]
    pub(crate) label: InternId,
}
