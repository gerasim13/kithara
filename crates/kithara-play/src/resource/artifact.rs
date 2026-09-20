use kithara_platform::sync::Arc;

use super::ResourceSrc;

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
#[derive(derive_more::Debug)]
#[derive_where::derive_where(Clone)]
#[non_exhaustive]
pub enum ArtifactSource<T> {
    /// The artifact itself, already built by the caller.
    #[debug("Value(..)")]
    Value(Arc<T>),
    /// A URL or local path the artifact's own bytes are read from. Never the
    /// audio source: an artifact has its own identity and its own format.
    Source(ResourceSrc),
}

impl<T> From<Arc<T>> for ArtifactSource<T> {
    fn from(value: Arc<T>) -> Self {
        Self::Value(value)
    }
}

impl<T> From<ResourceSrc> for ArtifactSource<T> {
    fn from(src: ResourceSrc) -> Self {
        Self::Source(src)
    }
}
