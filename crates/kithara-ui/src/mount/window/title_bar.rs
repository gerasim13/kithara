use bon::Builder;

use crate::{ids::InternId, size::SizeSpec};

/// The window's own caption strip, which also drags the window.
#[derive(Builder, kithara_derive::Control)]
#[control(size = SizeSpec::FILL)]
pub(crate) struct TitleBar {
    pub(crate) label: InternId,
}
