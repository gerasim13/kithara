use crate::{SessionFrame, WarpMapRevision};

/// An exact source/output boundary consumed by the audio callback.
#[derive(Clone, Copy, Debug, Eq, PartialEq, bon::Builder, fieldwork::Fieldwork)]
#[builder(state_mod(vis = "pub"))]
#[fieldwork(opt_in, get)]
#[non_exhaustive]
pub struct PresentationFrontier {
    /// Immutable producer map represented by this source/output boundary.
    #[field(get, copy, with)]
    warp_map: Option<WarpMapRevision>,
    /// Exclusive session output-frame boundary actually consumed.
    #[field(get, copy)]
    output: SessionFrame,
    /// Exclusive decoded source-frame boundary actually consumed.
    #[field(get, copy)]
    source: u64,
}
