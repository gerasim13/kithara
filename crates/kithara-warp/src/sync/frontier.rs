use crate::SessionFrame;

/// An exact source/output boundary reached by the renderer.
#[derive(Clone, Copy, Debug, Eq, PartialEq, bon::Builder, fieldwork::Fieldwork)]
#[builder(state_mod(vis = "pub"))]
#[fieldwork(opt_in, get)]
#[non_exhaustive]
pub struct RenderFrontier {
    /// Exclusive decoded source-frame boundary.
    #[field(get, copy)]
    source: u64,
    /// Exclusive session output-frame boundary.
    #[field(get, copy)]
    output: SessionFrame,
}

/// An exact source/output boundary consumed by the audio callback.
#[derive(Clone, Copy, Debug, Eq, PartialEq, bon::Builder, fieldwork::Fieldwork)]
#[builder(state_mod(vis = "pub"))]
#[fieldwork(opt_in, get)]
#[non_exhaustive]
pub struct PresentationFrontier {
    /// Exclusive decoded source-frame boundary actually consumed.
    #[field(get, copy)]
    source: u64,
    /// Exclusive session output-frame boundary actually consumed.
    #[field(get, copy)]
    output: SessionFrame,
}
