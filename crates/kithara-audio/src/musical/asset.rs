use std::num::NonZeroU32;

use arc_swap::ArcSwap;
use kithara_platform::sync::Arc;

use super::{
    AssetAxis, BeatMap, BeatMapId, BeatMapSnapshot, BeatMapSnapshotData, BeatMapSnapshotError,
    MapAxis, MapSegment, MapStamp, MapState, SegmentDraft, SegmentError, SegmentSet,
};

#[derive(Debug)]
struct AssetMapOwner {
    id: BeatMapId,
    axis: MapAxis,
    current: ArcSwap<BeatMapSnapshotData>,
}

/// Shared read handle for one evolving asset-native musical map.
#[derive(Clone, Debug)]
pub struct AssetBeatMap {
    owner: Arc<AssetMapOwner>,
}

/// Exclusive capability that publishes validated asset-map revisions.
#[derive(Debug)]
pub struct AssetMapPublisher {
    owner: Arc<AssetMapOwner>,
}

/// Complete normalized geometry for one asset-map publication.
///
/// `segments` replaces the previous snapshot as a whole; it is not a delta and
/// the publisher does not merge concurrent analyzer lanes. A stale producer
/// must rebuild a full candidate from the latest snapshot before retrying.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct AssetMapUpdate {
    base: MapStamp,
    state: MapState,
    segments: Vec<MapSegment>,
}

impl AssetMapUpdate {
    /// Creates a candidate based on one exact published snapshot.
    #[must_use]
    pub const fn new(base: MapStamp, state: MapState, segments: Vec<MapSegment>) -> Self {
        Self {
            base,
            state,
            segments,
        }
    }
}

impl TryFrom<(MapStamp, MapState, Vec<SegmentDraft>)> for AssetMapUpdate {
    type Error = SegmentError;

    fn try_from(
        (base, state, drafts): (MapStamp, MapState, Vec<SegmentDraft>),
    ) -> Result<Self, Self::Error> {
        let segments = drafts
            .into_iter()
            .map(SegmentDraft::validate)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self::new(base, state, segments))
    }
}

/// An asset-map candidate could not replace the current snapshot.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum AssetMapPublishError {
    /// The update targets another map identity or any non-current revision.
    #[error("asset map update is stale: expected {expected:?}, got {given:?}")]
    Stale { expected: MapStamp, given: MapStamp },
    /// A bounded asset map was assigned a live-domain lifecycle state.
    #[error("bounded asset map cannot publish state {state:?}")]
    InvalidState { state: MapState },
    /// A terminal asset-map state was reopened for later coverage.
    #[error("asset map cannot transition from {from:?} to {to:?}")]
    InvalidTransition { from: MapState, to: MapState },
    /// A complete map update changed its terminal coverage set.
    #[error("complete asset map update changed terminal coverage")]
    CoverageChanged,
    /// The candidate segments do not form valid geometry.
    #[error(transparent)]
    InvalidSegments(#[from] SegmentError),
    /// The map exhausted its monotonic revision space.
    #[error("asset map revision space is exhausted")]
    RevisionExhausted,
}

impl AssetBeatMap {
    /// Creates one asset map and its separate exclusive writer capability.
    ///
    /// The initial revision is [`MapState::Building`] with no covered regions.
    #[must_use]
    pub fn new(
        id: BeatMapId,
        source_sample_rate: NonZeroU32,
        source_frame_count: u64,
    ) -> (Self, AssetMapPublisher) {
        let asset_axis = AssetAxis::new(source_sample_rate, source_frame_count);
        let axis = MapAxis::Asset(asset_axis);
        let initial = BeatMapSnapshot::new_empty_asset(id, asset_axis);
        let owner = Arc::new(AssetMapOwner {
            id,
            axis,
            current: ArcSwap::from(initial.data),
        });
        (
            Self {
                owner: Arc::clone(&owner),
            },
            AssetMapPublisher { owner },
        )
    }
}

impl BeatMap for AssetBeatMap {
    fn id(&self) -> BeatMapId {
        self.owner.id
    }

    fn snapshot(&self) -> BeatMapSnapshot {
        BeatMapSnapshot::wrap(self.owner.current.load_full())
    }
}

impl AssetMapPublisher {
    /// Validates and atomically publishes the next immutable revision.
    ///
    /// # Errors
    ///
    /// Returns [`AssetMapPublishError`] when the base stamp is stale, the state
    /// or lifecycle transition is invalid, terminal coverage changes, segment
    /// geometry is invalid, or the revision counter is exhausted.
    pub fn publish(
        &mut self,
        update: AssetMapUpdate,
    ) -> Result<BeatMapSnapshot, AssetMapPublishError> {
        let current = BeatMapSnapshot::wrap(self.owner.current.load_full());
        let expected = current.stamp();
        if update.base != expected {
            return Err(AssetMapPublishError::Stale {
                expected,
                given: update.base,
            });
        }
        if current.state() == MapState::Complete && update.state != MapState::Complete {
            return Err(AssetMapPublishError::InvalidTransition {
                from: current.state(),
                to: update.state,
            });
        }
        let segments = SegmentSet::new(self.owner.axis, update.segments)?;
        if current.state() == MapState::Complete {
            match current.segments() {
                Some(previous) if previous.has_same_coverage(&segments) => {}
                Some(_) | None => return Err(AssetMapPublishError::CoverageChanged),
            }
        }
        let revision = current
            .revision()
            .checked_next()
            .ok_or(AssetMapPublishError::RevisionExhausted)?;
        let next = BeatMapSnapshot::try_from((self.owner.id, revision, update.state, segments))
            .map_err(|error| match error {
                BeatMapSnapshotError::InvalidState { state, .. } => {
                    AssetMapPublishError::InvalidState { state }
                }
            })?;
        self.owner.current.store(Arc::clone(&next.data));
        Ok(next)
    }
}
