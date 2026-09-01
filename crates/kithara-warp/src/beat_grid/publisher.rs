use kithara_platform::maybe_send::{MaybeSend, MaybeSync};

use super::{BeatGridId, BeatGridSnapshot};

/// Live publisher of immutable beat-grid snapshots.
///
/// Implementors are grid owners, not grid data: a player owns its decoder and
/// its reader, and on wasm those are bound to the worker that created them.
/// The bound is therefore `MaybeSend`, which is `Send` on every threaded
/// target and nothing on wasm.
pub trait BeatGrid: MaybeSend + MaybeSync + 'static {
    /// Returns the stable identity of this grid owner.
    fn id(&self) -> BeatGridId;

    /// Returns one immutable snapshot for a complete multi-step calculation.
    ///
    /// Implementors must preserve `snapshot().id() == id()`. Revisions for one
    /// grid identity never move backward, and every published replacement uses
    /// a later revision than the snapshot it replaces.
    fn snapshot(&self) -> BeatGridSnapshot;
}
