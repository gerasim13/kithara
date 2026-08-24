use std::num::NonZeroU64;

use kithara_platform::sync::Arc;
use portable_atomic::{AtomicU64, Ordering};

use super::{
    AssetAxis, Beat, BeatEvidence, BeatsPerMinute, FrameUncertainty, MapAxis, MapPoint,
    MapPosition, MapRegion, Meter, MeterFacts, SegmentSet, SessionAnchor, SessionBeat,
    SessionFrame,
};

const SECONDS_PER_MINUTE: f64 = 60.0;

/// Stable identity of one musical map owner.
#[derive(
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    derive_more::Display,
    derive_more::Into,
)]
#[display("{_0}")]
#[into(u64)]
#[repr(transparent)]
pub struct BeatMapId(NonZeroU64);

impl BeatMapId {
    /// Allocates an identity unique to this process.
    ///
    /// Every map owner uses this allocation site so points from independent
    /// sessions and registries cannot acquire equal stamps accidentally.
    ///
    /// # Errors
    ///
    /// Returns [`BeatMapIdAllocationError`] after the non-zero identity space
    /// has been exhausted.
    pub fn allocate() -> Result<Self, BeatMapIdAllocationError> {
        static NEXT: AtomicU64 = AtomicU64::new(1);

        let value = NEXT
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                (current != 0).then(|| current.wrapping_add(1))
            })
            .map_err(|_| BeatMapIdAllocationError)?;
        NonZeroU64::new(value)
            .map(Self)
            .ok_or(BeatMapIdAllocationError)
    }
}

/// The process-wide musical-map identity space is exhausted.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("beat map identity space is exhausted")]
pub struct BeatMapIdAllocationError;

/// Monotonic revision of one [`BeatMapId`].
#[derive(
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    derive_more::Display,
    derive_more::Into,
)]
#[display("{_0}")]
#[into(u64)]
#[repr(transparent)]
pub struct BeatMapRevision(NonZeroU64);

impl BeatMapRevision {
    /// Returns the first revision assigned by a map owner.
    #[must_use]
    pub const fn first() -> Self {
        Self(NonZeroU64::MIN)
    }

    /// Returns the next owner-assigned revision, or `None` on exhaustion.
    #[must_use]
    pub fn checked_next(self) -> Option<Self> {
        self.0
            .get()
            .checked_add(1)
            .and_then(NonZeroU64::new)
            .map(Self)
    }
}

/// Identity and immutable revision of one map snapshot.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub struct MapStamp {
    map_id: BeatMapId,
    revision: BeatMapRevision,
}

impl MapStamp {
    /// Creates a composite map stamp.
    #[must_use]
    pub const fn new(map_id: BeatMapId, revision: BeatMapRevision) -> Self {
        Self { map_id, revision }
    }

    /// Returns the stable map identity.
    #[must_use]
    pub const fn map_id(self) -> BeatMapId {
        self.map_id
    }

    /// Returns the immutable map revision.
    #[must_use]
    pub const fn revision(self) -> BeatMapRevision {
        self.revision
    }
}

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
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct BeatEstimate<T> {
    value: T,
    evidence: BeatEvidence,
    uncertainty: FrameUncertainty,
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

    /// Returns the resolved value.
    #[must_use]
    pub const fn value(&self) -> &T {
        &self.value
    }

    /// Returns how the value was established.
    #[must_use]
    pub const fn evidence(&self) -> BeatEvidence {
        self.evidence
    }

    /// Returns maximum absolute error in map-native frames.
    #[must_use]
    pub const fn uncertainty(&self) -> FrameUncertainty {
        self.uncertainty
    }

    /// Returns the snapshot identity and revision used by the estimate.
    #[must_use]
    pub const fn stamp(&self) -> MapStamp {
        self.stamp
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

/// Read-only musical-coordinate protocol shared by asset and host maps.
pub trait BeatMap: Send + Sync + 'static {
    /// Returns the stable identity of this map owner.
    fn id(&self) -> BeatMapId;

    /// Returns one immutable snapshot for a complete multi-step calculation.
    ///
    /// Implementors must preserve `snapshot().id() == id()`. Revisions for one
    /// map identity never move backward, and every published replacement uses
    /// a later revision than the snapshot it replaces.
    fn snapshot(&self) -> BeatMapSnapshot;
}

/// One immutable, revisioned musical-map observation.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct BeatMapSnapshot {
    pub(crate) data: Arc<BeatMapSnapshotData>,
}

/// A caller-supplied snapshot violates the public musical-map contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum BeatMapSnapshotError {
    /// The lifecycle state is incompatible with the snapshot coordinate axis.
    #[error("state {state:?} is invalid for segment geometry on axis {axis:?}")]
    InvalidState { axis: MapAxis, state: MapState },
}

#[derive(Debug)]
pub(crate) struct BeatMapSnapshotData {
    pub(crate) id: BeatMapId,
    pub(crate) revision: BeatMapRevision,
    pub(crate) state: MapState,
    pub(crate) axis: MapAxis,
    pub(crate) geometry: BeatMapGeometry,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum BeatMapGeometry {
    Segments(SegmentSet),
    Host {
        anchor: SessionAnchor,
        meter: Option<MeterFacts>,
    },
}

/// Creates a snapshot from independently owned, validated segment geometry.
///
/// This is the construction boundary for external [`BeatMap`] owners such as
/// synchronization groups. The segment set preserves its validated axis, so a
/// caller cannot pair topology with a different coordinate domain.
impl TryFrom<(BeatMapId, BeatMapRevision, MapState, SegmentSet)> for BeatMapSnapshot {
    type Error = BeatMapSnapshotError;

    fn try_from(
        (id, revision, state, segments): (BeatMapId, BeatMapRevision, MapState, SegmentSet),
    ) -> Result<Self, Self::Error> {
        let axis = segments.axis();
        if matches!(
            (axis, state),
            (MapAxis::Asset(_), MapState::Live)
                | (MapAxis::Host(_), MapState::Complete)
                | (
                    _,
                    MapState::Unavailable(MapUnavailable::AxisMismatch | MapUnavailable::NoMeter)
                )
        ) {
            return Err(BeatMapSnapshotError::InvalidState { axis, state });
        }
        Ok(Self::new_segments(id, revision, state, segments))
    }
}

impl BeatMapSnapshot {
    /// Copies this immutable geometry under a caller-owned map stamp.
    #[must_use]
    pub fn restamp(&self, stamp: MapStamp) -> Self {
        Self {
            data: Arc::new(BeatMapSnapshotData {
                id: stamp.map_id(),
                revision: stamp.revision(),
                state: self.state(),
                axis: self.axis(),
                geometry: self.data.geometry.clone(),
            }),
        }
    }

    fn new_segments(
        id: BeatMapId,
        revision: BeatMapRevision,
        state: MapState,
        segments: SegmentSet,
    ) -> Self {
        let axis = segments.axis();
        Self {
            data: Arc::new(BeatMapSnapshotData {
                id,
                revision,
                state,
                axis,
                geometry: BeatMapGeometry::Segments(segments),
            }),
        }
    }

    pub(crate) fn new_empty_asset(id: BeatMapId, axis: AssetAxis) -> Self {
        Self::new_segments(
            id,
            BeatMapRevision::first(),
            MapState::Building,
            SegmentSet::empty(MapAxis::Asset(axis)),
        )
    }

    pub(crate) fn new_host(
        id: BeatMapId,
        revision: BeatMapRevision,
        axis: MapAxis,
        anchor: SessionAnchor,
        meter: Option<MeterFacts>,
    ) -> Self {
        Self {
            data: Arc::new(BeatMapSnapshotData {
                id,
                revision,
                state: MapState::Live,
                axis,
                geometry: BeatMapGeometry::Host { anchor, meter },
            }),
        }
    }

    pub(crate) fn wrap(data: Arc<BeatMapSnapshotData>) -> Self {
        Self { data }
    }

    delegate::delegate! {
        to self.data {
            /// Returns the stable map identity.
            #[must_use]
            #[field]
            pub fn id(&self) -> BeatMapId;
            /// Returns the immutable map revision.
            #[must_use]
            #[field]
            pub fn revision(&self) -> BeatMapRevision;
            /// Returns the snapshot lifecycle state.
            #[must_use]
            #[field]
            pub fn state(&self) -> MapState;
            /// Returns the typed coordinate axis used by this snapshot.
            #[must_use]
            #[field]
            pub fn axis(&self) -> MapAxis;
        }
    }

    /// Returns the composite identity and revision.
    #[must_use]
    pub fn stamp(&self) -> MapStamp {
        MapStamp::new(self.id(), self.revision())
    }

    /// Returns the validated immutable segment collection for a segment-backed map.
    #[must_use]
    pub fn segments(&self) -> Option<&SegmentSet> {
        match &self.data.geometry {
            BeatMapGeometry::Segments(segments) => Some(segments),
            BeatMapGeometry::Host { .. } => None,
        }
    }

    /// Resolves a stamped map-native position to a stamped beat.
    pub fn beat_at(
        &self,
        position: MapPoint<MapPosition>,
    ) -> MapQuery<BeatEstimate<MapPoint<Beat>>> {
        if let Some(stale) = self.stale(position.stamp()) {
            return stale;
        }
        if position.value().kind() != self.axis().kind() {
            return MapQuery::Unavailable(MapUnavailable::AxisMismatch);
        }
        if let MapState::Unavailable(reason) = self.state() {
            return MapQuery::Unavailable(reason);
        }
        if self.outside_asset_extent(*position.value()) {
            return MapQuery::OutsideDomain;
        }
        match &self.data.geometry {
            BeatMapGeometry::Segments(segments) => {
                let Some((beat, evidence, uncertainty)) = segments
                    .by_position(*position.value())
                    .and_then(|segment| segment.beat_at(*position.value()))
                else {
                    return self.missing_position(*position.value());
                };
                MapQuery::Resolved(BeatEstimate::new(
                    MapPoint::new(self.stamp(), beat),
                    evidence,
                    uncertainty,
                    self.stamp(),
                ))
            }
            BeatMapGeometry::Host { anchor, .. } => {
                let MapPosition::Host(frame) = *position.value() else {
                    return MapQuery::Unavailable(MapUnavailable::AxisMismatch);
                };
                let Ok(session_beat) = anchor.beat_at(frame) else {
                    return MapQuery::OutsideDomain;
                };
                let Ok(beat) = Beat::new(f64::from(session_beat)) else {
                    return MapQuery::OutsideDomain;
                };
                MapQuery::Resolved(BeatEstimate::new(
                    MapPoint::new(self.stamp(), beat),
                    BeatEvidence::Declared,
                    FrameUncertainty::ZERO,
                    self.stamp(),
                ))
            }
        }
    }

    /// Resolves a stamped beat to a stamped map-native position.
    pub fn position_at(
        &self,
        beat: MapPoint<Beat>,
    ) -> MapQuery<BeatEstimate<MapPoint<MapPosition>>> {
        if let Some(stale) = self.stale(beat.stamp()) {
            return stale;
        }
        if let MapState::Unavailable(reason) = self.state() {
            return MapQuery::Unavailable(reason);
        }
        match &self.data.geometry {
            BeatMapGeometry::Segments(segments) => {
                let Some((position, evidence, uncertainty)) = segments
                    .by_beat(*beat.value())
                    .and_then(|segment| segment.position_at(*beat.value()))
                else {
                    return self.missing_beat(*beat.value());
                };
                MapQuery::Resolved(BeatEstimate::new(
                    MapPoint::new(self.stamp(), position),
                    evidence,
                    uncertainty,
                    self.stamp(),
                ))
            }
            BeatMapGeometry::Host { anchor, .. } => {
                let Ok(session_beat) = SessionBeat::new(f64::from(*beat.value())) else {
                    return MapQuery::OutsideDomain;
                };
                let Ok(frame) = anchor.frame_at(session_beat) else {
                    return MapQuery::OutsideDomain;
                };
                let Ok(rounded_beat) = anchor.beat_at(frame) else {
                    return MapQuery::OutsideDomain;
                };
                let residual_frames = ((f64::from(session_beat) - f64::from(rounded_beat))
                    / anchor.beats_per_frame())
                .abs();
                let Ok(uncertainty) = FrameUncertainty::new(residual_frames) else {
                    return MapQuery::OutsideDomain;
                };
                MapQuery::Resolved(BeatEstimate::new(
                    MapPoint::new(self.stamp(), MapPosition::Host(frame)),
                    BeatEvidence::Declared,
                    uncertainty,
                    self.stamp(),
                ))
            }
        }
    }

    /// Resolves the local tempo derived from the same segment topology.
    pub fn tempo_at(
        &self,
        position: MapPoint<MapPosition>,
    ) -> MapQuery<BeatEstimate<BeatsPerMinute>> {
        if let Some(stale) = self.stale(position.stamp()) {
            return stale;
        }
        if position.value().kind() != self.axis().kind() {
            return MapQuery::Unavailable(MapUnavailable::AxisMismatch);
        }
        if let MapState::Unavailable(reason) = self.state() {
            return MapQuery::Unavailable(reason);
        }
        if self.outside_asset_extent(*position.value()) {
            return MapQuery::OutsideDomain;
        }
        match &self.data.geometry {
            BeatMapGeometry::Segments(segments) => {
                let Some((tempo, evidence, uncertainty)) = segments
                    .by_position(*position.value())
                    .and_then(|segment| segment.tempo_at(self.axis(), *position.value()))
                else {
                    return self.missing_position(*position.value());
                };
                MapQuery::Resolved(BeatEstimate::new(
                    tempo,
                    evidence,
                    uncertainty,
                    self.stamp(),
                ))
            }
            BeatMapGeometry::Host { anchor, .. } => {
                let bpm = anchor.beats_per_second() * SECONDS_PER_MINUTE;
                let Some(tempo) = BeatsPerMinute::new(bpm) else {
                    return MapQuery::Unavailable(MapUnavailable::NoGeometry);
                };
                MapQuery::Resolved(BeatEstimate::new(
                    tempo,
                    BeatEvidence::Declared,
                    FrameUncertainty::ZERO,
                    self.stamp(),
                ))
            }
        }
    }

    /// Resolves the meter carried by the segment containing `beat`.
    pub fn meter_at(&self, beat: MapPoint<Beat>) -> MapQuery<BeatEstimate<Meter>> {
        if let Some(stale) = self.stale(beat.stamp()) {
            return stale;
        }
        if let MapState::Unavailable(reason) = self.state() {
            return MapQuery::Unavailable(reason);
        }
        match &self.data.geometry {
            BeatMapGeometry::Segments(segments) => {
                let Some(segment) = segments.by_beat(*beat.value()) else {
                    return self.missing_beat(*beat.value());
                };
                let Some((meter, evidence, uncertainty)) = segment.meter_at(*beat.value()) else {
                    return self.missing_meter(segment.region());
                };
                MapQuery::Resolved(BeatEstimate::new(
                    meter,
                    evidence,
                    uncertainty,
                    self.stamp(),
                ))
            }
            BeatMapGeometry::Host { meter, .. } => {
                let Some(meter) = *meter else {
                    return MapQuery::Unavailable(MapUnavailable::NoMeter);
                };
                let (value, evidence, uncertainty) = meter.into_parts();
                MapQuery::Resolved(BeatEstimate::new(
                    value,
                    evidence,
                    uncertainty,
                    self.stamp(),
                ))
            }
        }
    }

    fn stale<T>(&self, given: MapStamp) -> Option<MapQuery<T>> {
        let expected = self.stamp();
        (given != expected).then_some(MapQuery::Stale { expected, given })
    }

    fn missing_position<T>(&self, position: MapPosition) -> MapQuery<T> {
        match self.state() {
            MapState::Complete => MapQuery::OutsideDomain,
            MapState::Building | MapState::Live => MapQuery::Uncovered {
                required: match &self.data.geometry {
                    BeatMapGeometry::Segments(segments) => segments.uncovered_region(position),
                    BeatMapGeometry::Host { .. } => MapRegion::point(position),
                },
            },
            MapState::Unavailable(reason) => MapQuery::Unavailable(reason),
        }
    }

    fn missing_beat<T>(&self, beat: Beat) -> MapQuery<T> {
        match self.state() {
            MapState::Complete => MapQuery::OutsideDomain,
            MapState::Building | MapState::Live => MapQuery::Uncovered {
                required: match &self.data.geometry {
                    BeatMapGeometry::Segments(segments) => segments.uncovered_region_by_beat(beat),
                    BeatMapGeometry::Host { .. } => {
                        MapRegion::point(MapPosition::Host(SessionFrame::new(0)))
                    }
                },
            },
            MapState::Unavailable(reason) => MapQuery::Unavailable(reason),
        }
    }

    fn missing_meter<T>(&self, required: MapRegion) -> MapQuery<T> {
        match self.state() {
            MapState::Building => MapQuery::Uncovered { required },
            MapState::Complete | MapState::Live => MapQuery::Unavailable(MapUnavailable::NoMeter),
            MapState::Unavailable(reason) => MapQuery::Unavailable(reason),
        }
    }

    fn outside_asset_extent(&self, position: MapPosition) -> bool {
        match (self.axis(), position) {
            (MapAxis::Asset(axis), MapPosition::Asset(frame)) => !axis.contains(frame),
            _ => false,
        }
    }
}

impl PartialEq for BeatMapSnapshot {
    fn eq(&self, other: &Self) -> bool {
        self.id() == other.id()
            && self.revision() == other.revision()
            && self.state() == other.state()
            && self.axis() == other.axis()
            && self.data.geometry == other.data.geometry
    }
}
