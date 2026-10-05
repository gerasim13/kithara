use crate::{shader::ShaderSpec, size::SizeSpec};

/// A document-owned shader that occupies its declared layout box.
#[derive(kithara_derive::Control)]
#[control(size = SizeSpec::FILL)]
pub(crate) struct Shader<#[cfg(any(feature = "iced", feature = "masonry"))] 'a> {
    #[cfg(any(feature = "iced", feature = "masonry"))]
    pub(crate) spec: &'a ShaderSpec,
}

#[cfg(any(feature = "iced", feature = "masonry"))]
impl<'a> Shader<'a> {
    pub(crate) const fn new(spec: &'a ShaderSpec) -> Self {
        Self { spec }
    }
}

#[cfg(not(any(feature = "iced", feature = "masonry")))]
impl Shader {
    pub(crate) const fn new(_spec: &ShaderSpec) -> Self {
        Self {}
    }
}
