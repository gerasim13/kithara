use std::fmt::Debug;

use super::{
    BeatEstimate, BeatGridId, BeatGridQuery, BeatGridRegion, BeatGridRevision, BeatGridStamp,
    BeatGridState,
};
use crate::{Beat, BeatsPerMinute, MapAxis, MapPoint, MapPosition, Meter};

/// One immutable, revisioned view of musical timing facts.
///
/// Every observable answer, including `state` and `axis`, must remain stable
/// for the lifetime of the view. New or refined facts require a new revision
/// and a new view.
pub trait BeatGridView: Debug + Send + Sync + 'static {
    /// Returns the stable identity of the owning live grid.
    fn id(&self) -> BeatGridId;

    /// Returns the immutable revision represented by this view.
    fn revision(&self) -> BeatGridRevision;

    /// Returns the lifecycle state represented by this view.
    fn state(&self) -> BeatGridState;

    /// Returns the native coordinate axis used by this view.
    fn axis(&self) -> MapAxis;

    /// Returns the composite identity and revision.
    fn stamp(&self) -> BeatGridStamp {
        BeatGridStamp::new(self.id(), self.revision())
    }

    /// Resolves the affine region containing a stamped native position.
    fn region_at(&self, position: MapPoint<MapPosition>) -> BeatGridQuery<BeatGridRegion>;

    /// Resolves a stamped native position to a stamped beat.
    fn beat_at(
        &self,
        position: MapPoint<MapPosition>,
    ) -> BeatGridQuery<BeatEstimate<MapPoint<Beat>>>;

    /// Resolves a stamped beat to a stamped native position.
    fn position_at(
        &self,
        beat: MapPoint<Beat>,
    ) -> BeatGridQuery<BeatEstimate<MapPoint<MapPosition>>>;

    /// Resolves local tempo at a stamped native position.
    fn tempo_at(
        &self,
        position: MapPoint<MapPosition>,
    ) -> BeatGridQuery<BeatEstimate<BeatsPerMinute>>;

    /// Resolves meter at a stamped beat.
    fn meter_at(&self, beat: MapPoint<Beat>) -> BeatGridQuery<BeatEstimate<Meter>>;
}
