use crate::size::SizeSpec;

/// A document-owned shader that occupies its declared layout box.
#[derive(kithara_derive::Control)]
#[control(size = SizeSpec::FILL)]
pub(crate) struct Shader;

#[cfg(any(feature = "iced", feature = "masonry"))]
pub(crate) mod host {
    use bon::Builder;

    use crate::shader::ShaderSpec;

    #[derive(Builder)]
    pub(crate) struct Shader<'a> {
        pub(crate) spec: &'a ShaderSpec,
    }

    impl<'a> Shader<'a> {
        pub(crate) const fn new(spec: &'a ShaderSpec) -> Self {
            Self { spec }
        }
    }
}
