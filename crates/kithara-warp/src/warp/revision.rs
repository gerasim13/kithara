use kithara_signal::Revision;

/// The revision domain for immutable warp maps.
pub enum WarpMapRevisionTag {}

/// Monotonic revision of one immutable warp map.
pub type WarpMapRevision = Revision<WarpMapRevisionTag>;
