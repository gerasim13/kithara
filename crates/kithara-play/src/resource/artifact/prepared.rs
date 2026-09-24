use kithara_platform::sync::Arc;

use super::{ArtifactDocument, ArtifactFetch, ArtifactLoadError};
use crate::resource::ResourceSrc;

/// How a prepared artifact reaches a resource: a structure the caller already
/// holds, or an external source this resource loads it from.
///
/// One enum serves every artifact kind, because the choice is the same choice
/// each time and both variants converge on one validated result: a consumer of
/// a publication never learns which one the track was opened with.
///
/// The value is held behind an [`Arc`] because a resource configuration is
/// cloned per load, per queue forward, and per analysis entry. A full-track
/// grid or waveform must not be copied on any of those.
#[derive(derive_more::Debug, derive_more::From)]
#[derive_where::derive_where(Clone)]
#[non_exhaustive]
pub enum ArtifactSource<T> {
    /// The artifact itself, already built by the caller.
    #[debug("Value(..)")]
    #[from]
    Value(Arc<T>),
    /// A URL or local path the artifact's own bytes are read from. Never the
    /// audio source: an artifact has its own identity and its own format.
    #[from]
    Source(ResourceSrc),
}

impl<T: ArtifactDocument> ArtifactSource<T> {
    /// The artifact itself, reading it from its source first when that is how
    /// the track was opened.
    ///
    /// A value costs nothing and performs no I/O; a source rides `fetch`, so
    /// it is cancelled with the load it belongs to.
    ///
    /// # Errors
    ///
    /// Returns why an external artifact never arrived or did not parse. There
    /// is no fall back to local analysis: a track opened with an explicit
    /// source asked for that artifact.
    pub async fn load(&self, fetch: &ArtifactFetch<'_>) -> Result<Arc<T>, ArtifactLoadError> {
        match self {
            Self::Value(value) => Ok(Arc::clone(value)),
            Self::Source(src) => fetch.load(src).await,
        }
    }
}
