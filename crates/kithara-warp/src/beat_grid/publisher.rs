use super::{BeatGridId, BeatGridSnapshot};

/// Live publisher of immutable beat-grid snapshots.
pub trait BeatGrid: Send + Sync + 'static {
    /// Returns the stable identity of this grid owner.
    fn id(&self) -> BeatGridId;

    /// Returns one immutable snapshot for a complete multi-step calculation.
    ///
    /// Implementors must preserve `snapshot().id() == id()`. Revisions for one
    /// grid identity never move backward, and every published replacement uses
    /// a later revision than the snapshot it replaces.
    fn snapshot(&self) -> BeatGridSnapshot;
}
