use super::{BeatMapId, BeatMapSnapshot};
use crate::{AlignmentPlan, AlignmentRequest, PlanTransition, PresentationFrontier, SyncError};

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

    /// Compiles a stamped source-to-target alignment plan.
    ///
    /// # Errors
    ///
    /// Returns [`SyncError`] when map coverage, stamps, coordinates, policy, or
    /// the implementation's alignment capability cannot satisfy `request`.
    fn align_to(
        &self,
        target: &dyn BeatMap,
        request: AlignmentRequest,
    ) -> Result<AlignmentPlan, SyncError>;

    /// Reconciles a newer map observation without changing already audible PCM.
    ///
    /// # Errors
    ///
    /// Returns [`SyncError`] when the active plan or frontier is stale, or a
    /// continuity-preserving successor cannot be compiled.
    fn reconcile_to(
        &self,
        target: &dyn BeatMap,
        active: &AlignmentPlan,
        frontier: PresentationFrontier,
    ) -> Result<PlanTransition, SyncError>;
}
