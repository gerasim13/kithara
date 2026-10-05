use kithara_platform::{CancelToken, tokio::runtime::Handle};
use serde::de::DeserializeOwned;
use serde_yaml_ng::Value;

use crate::Registration;

/// What a source is built from: the application's shared HTTP client, its
/// runtime, a cancellation of its own and its entry of the document's
/// `sources` map, references already resolved.
pub struct Context<N> {
    pub cancel: CancelToken,
    pub runtime: Handle,
    pub net: N,
    id: &'static str,
    section: Value,
}

impl<N> Context<N> {
    #[must_use]
    pub const fn new(
        id: &'static str,
        net: N,
        runtime: Handle,
        cancel: CancelToken,
        section: Value,
    ) -> Self {
        Self {
            cancel,
            runtime,
            net,
            id,
            section,
        }
    }

    /// The source's entry in the schema the source owns.
    ///
    /// # Errors
    /// Returns [`SectionError`] when the entry does not match that schema.
    pub fn section<T: DeserializeOwned>(&self) -> Result<T, SectionError> {
        T::deserialize(&self.section).map_err(|_| SectionError { id: self.id })
    }
}

/// A `sources` entry its source cannot read. It names the entry and never the
/// value, which may hold a resolved secret.
#[derive(Debug, thiserror::Error)]
#[error("sources.{id} does not match its source's schema")]
pub struct SectionError {
    id: &'static str,
}

/// A source the application can mount, keyed by the `sources` entry it reads.
pub struct Factory<N> {
    /// The source's id, which names its `sources` entry.
    pub id: &'static str,
    /// Builds the source's registration from its context.
    pub register: fn(Context<N>) -> Result<Registration, SectionError>,
}
