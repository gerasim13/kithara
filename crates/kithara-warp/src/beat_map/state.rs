use super::MapStamp;
use crate::{BeatEvidence, FrameUncertainty, MapRegion};

/// Why a map cannot currently answer queries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum MapUnavailable {
    /// The supplied coordinate uses another map-native axis.
    AxisMismatch,
    /// No usable geometry is available for this map.
    NoGeometry,
    /// Beat geometry exists, but no meter evidence is available.
    NoMeter,
}

/// Lifecycle state of one immutable map snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum MapState {
    /// Partial geometry may gain coverage in later revisions.
    Building,
    /// The bounded map will not gain more coverage.
    Complete,
    /// The map describes a live coordinate domain.
    Live,
    /// The map cannot currently answer any query.
    Unavailable(MapUnavailable),
}

/// A resolved value and the evidence supporting it.
#[derive(Clone, Debug, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
#[non_exhaustive]
pub struct BeatEstimate<T> {
    /// Returns the resolved value.
    #[field(get)]
    value: T,
    /// Returns how the value was established.
    #[field(get, copy)]
    evidence: BeatEvidence,
    /// Returns maximum absolute error in map-native frames.
    #[field(get, copy)]
    uncertainty: FrameUncertainty,
    /// Returns the snapshot identity and revision used by the estimate.
    #[field(get, copy)]
    stamp: MapStamp,
}

impl<T> BeatEstimate<T> {
    pub(crate) const fn new(
        value: T,
        evidence: BeatEvidence,
        uncertainty: FrameUncertainty,
        stamp: MapStamp,
    ) -> Self {
        Self {
            value,
            evidence,
            uncertainty,
            stamp,
        }
    }
}

/// Typed result of a musical-map query.
#[derive(Clone, Debug, PartialEq)]
#[must_use]
#[non_exhaustive]
pub enum MapQuery<T> {
    /// The snapshot resolved the requested coordinate.
    Resolved(T),
    /// A later revision may cover the requested map region.
    Uncovered { required: MapRegion },
    /// The complete bounded map proves the coordinate is outside its domain.
    OutsideDomain,
    /// The supplied point belongs to another map identity or revision.
    Stale { expected: MapStamp, given: MapStamp },
    /// The map cannot answer the query for the stated reason.
    Unavailable(MapUnavailable),
}
