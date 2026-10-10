use crate::size::SizeSpec;

/// The window's own caption strip, which also drags the window.
#[derive(kithara_derive::Control)]
#[control(size = SizeSpec::FILL)]
pub(crate) struct TitleBar;

#[cfg(any(feature = "iced", feature = "masonry"))]
pub(crate) mod host {
    use bon::Builder;

    use crate::ids::InternId;

    #[derive(Builder)]
    pub(crate) struct TitleBar {
        pub(crate) label: InternId,
    }
}
