use crate::{SessionFrame, WarpMapRevision};

/// Renderer-local progress through one immutable [`super::WarpMap`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
#[non_exhaustive]
pub struct WarpCursor {
    /// Exclusive session-output boundary already rendered.
    #[field(get, copy)]
    output: SessionFrame,
    /// Immutable map revision this progress belongs to.
    #[field(get, copy)]
    revision: WarpMapRevision,
    /// Exclusive decoded-source boundary already rendered.
    #[field(get, copy)]
    source: u64,
}

impl WarpCursor {
    pub(super) const fn new(revision: WarpMapRevision, source: u64, output: SessionFrame) -> Self {
        Self {
            output,
            revision,
            source,
        }
    }
}
