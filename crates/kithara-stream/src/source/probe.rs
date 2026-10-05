use std::ops::Range;

use kithara_platform::sync::Arc;

use crate::{ByteMap, SourcePhase};

/// Narrow view of the source's byte space, served off the control mutex.
///
/// Phase, cursor, length, and byte map are non-blocking `&self` snapshots
/// on [`crate::Source`], but reaching them through a `Mutex<Stream>` turns each
/// query into a lock acquisition — off-limits on the forbid-blocking audio
/// produce core, where any contended acquire (a construction read or a
/// consumer query holding the control mutex) blocks the real-time tick.
/// Callers on that core take this handle once at open and answer every
/// byte-space question without touching the stream's control-plane mutex.
/// Implementations answer from self-synchronizing state and must not take
/// locks a reader wait can hold.
pub trait SourceProbe: Send + Sync + 'static {
    /// Optional byte-map handle — the same answer as [`crate::Source::byte_map`],
    /// including its over-time `None` → `Some` transition.
    fn byte_map(&self) -> Option<Arc<dyn ByteMap>>;

    /// Whether the source currently reports zero bytes — the same
    /// convention as [`crate::Source::is_empty`].
    fn is_empty(&self) -> bool {
        self.len().is_none_or(|n| n == 0)
    }

    /// Total length if known — the same answer as [`crate::Source::len`].
    fn len(&self) -> Option<u64>;

    /// Overall source readiness at the current position.
    fn phase(&self) -> SourcePhase;

    /// Point-in-time readiness for a specific byte range.
    fn phase_at(&self, range: Range<u64>) -> SourcePhase;

    /// Current read position — the same atomic cursor as
    /// [`crate::Source::position`].
    fn position(&self) -> u64;

    /// Absolute byte-position set — the same atomic cursor as
    /// [`crate::Source::set_position`].
    fn set_position(&self, pos: u64);
}
