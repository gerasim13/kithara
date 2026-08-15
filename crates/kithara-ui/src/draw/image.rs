use kithara_platform::sync::Arc;

/// Stable identity of an image owned outside the neutral draw list.
///
/// The producer owns the pixels or GPU texture. The list carries only this
/// identity and the destination rectangle, so no backend resource type crosses
/// the draw seam.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ImageId(Arc<str>);

impl ImageId {
    /// Creates an image identity within its producer's resource registry.
    #[must_use]
    pub fn new(id: &str) -> Self {
        Self(Arc::from(id))
    }
}
