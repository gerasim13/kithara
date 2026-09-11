use kithara_platform::sync::Arc;
#[cfg(feature = "usdt")]
use kithara_test_macros as kithara;
use kithara_warp::{
    AlignmentSource, BeatGrid, BeatGridId, BeatGridRevision, BeatGridSnapshot, BeatGridState,
    ReconcileCause, SegmentSet, SyncAdmission, SyncApplied, SyncError, SyncGroup, SyncMember,
    SyncOperation, SyncRejected, SyncStatusSnapshot, TopologyOperation, WarpMap,
};
use tracing::warn;

use super::PlayerImpl;
use crate::{
    api::TrackId,
    player::{protocol::PlayerMember, state::TrackGrid},
};

/// One published asset grid of a queued track, boxed as a topology member.
struct TrackGridMember(BeatGridSnapshot);

impl BeatGrid for TrackGridMember {
    delegate::delegate! {
        to self.0 {
            fn id(&self) -> BeatGridId;
            #[call(clone)]
            fn snapshot(&self) -> BeatGridSnapshot;
        }
    }
}

impl<S> PlayerImpl<S>
where
    S: Send + Sync + 'static,
{
    /// Publishes the asset grid of one queued track on this deck's sync group
    /// and reconciles the track onto the deck.
    ///
    /// A first publication allocates the grid identity and attaches it; a later
    /// one replaces the member under the next revision.
    ///
    /// # Errors
    ///
    /// Returns the group's rejection or the exhausted identity space.
    pub(crate) fn publish_item_grid(
        &mut self,
        item: TrackId,
        segments: SegmentSet,
        state: BeatGridState,
    ) -> Result<SyncAdmission, SyncError> {
        let previous = self.runtime.core.items.track_grid(item);
        let (id, revision, cause) = match &previous {
            Some(grid) => (
                grid.id,
                grid.revision
                    .checked_next()
                    .ok_or(SyncError::BeatGridRevisionExhausted { grid_id: grid.id })?,
                ReconcileCause::GridRefined,
            ),
            None => (
                BeatGridId::allocate()?,
                BeatGridRevision::first(),
                ReconcileCause::GridAvailable,
            ),
        };
        let snapshot = BeatGridSnapshot::segments(id, revision, state, segments.clone())?;
        let base = self.sync.topology()?.stamp();
        let member = SyncMember::Grid {
            alignment: None,
            grid: Box::new(TrackGridMember(snapshot)),
        };
        let operation = if previous.is_some() {
            TopologyOperation::Replace {
                member: id,
                replacement: member,
            }
        } else {
            TopologyOperation::Attach { member }
        };
        let _ = self.transact_sync(SyncOperation::Topology {
            base,
            operations: Box::new([operation]),
        })?;
        self.runtime.core.items.publish_track_grid(
            item,
            TrackGrid {
                id,
                revision,
                segments,
            },
        );
        self.replan_track(item);
        self.reconcile_item_grid(item, cause, None)
            .map_err(|rejected| {
                let (error, _) = rejected.into();
                error
            })?
            .ok_or_else(|| SyncError::MemberNotFound {
                group_id: self.sync.id(),
                member_id: id,
            })
    }

    pub(crate) fn reconcile_current_grid(
        &mut self,
        cause: ReconcileCause,
        source: Option<AlignmentSource>,
    ) -> Result<Option<SyncAdmission>, SyncRejected<PlayerMember>> {
        let Some(item) = self.runtime.core.items.current_item_id() else {
            return Ok(None);
        };
        self.reconcile_item_grid(item, cause, source)
    }

    fn reconcile_item_grid(
        &mut self,
        item: TrackId,
        cause: ReconcileCause,
        source: Option<AlignmentSource>,
    ) -> Result<Option<SyncAdmission>, SyncRejected<PlayerMember>> {
        let Some(grid) = self.runtime.core.items.track_grid(item) else {
            return Ok(None);
        };
        let (load, transport) = {
            let sync = &self.sync;
            #[cfg(target_arch = "wasm32")]
            let sync = sync.owned()?;
            sync.generations()
        };
        let frontier = self.runtime.presentation_frontier();
        let source = source.unwrap_or_else(|| {
            self.runtime.playback_snapshot().map_or(
                AlignmentSource::Prepared(frontier),
                |snapshot| {
                    if snapshot.is_playing() {
                        AlignmentSource::Audible {
                            presentation: frontier,
                            preparation_source: snapshot.preparation_source(
                                frontier.source(),
                                self.runtime.core.response_budget_frames,
                            ),
                            playback_rate: kithara_warp::RateTarget::default()
                                .with_speed(snapshot.rate),
                        }
                    } else {
                        AlignmentSource::Prepared(frontier)
                    }
                },
            )
        });
        let admission = self.sync.transact(SyncOperation::Reconcile {
            target: grid.id,
            load,
            transport,
            cause,
            source,
        })?;
        let prepared = {
            let sync = &self.sync;
            #[cfg(target_arch = "wasm32")]
            let sync = sync.owned()?;
            sync.prepared()
        };
        if let Some(prepared) = prepared.filter(|prepared| prepared.target == grid.id)
            && let Some(grid) = self.runtime.core.items.track_grid(item)
        {
            #[cfg(feature = "usdt")]
            {
                kithara::probe_event!(
                    warp_plan_published,
                    warp_map_revision = u64::from(prepared.warp_map),
                    presentation_source = source.frontier().source(),
                    preparation_source = source.preparation_source(),
                    activation_source = prepared.source,
                    activation_output = u64::try_from(i64::from(prepared.activation)).unwrap_or(0)
                );
            }
            let plan = grid
                .segments
                .region_plan()
                .inspect_err(|error| warn!(%error, %item, "track grid has no region plan"))
                .ok()
                .map(|plan| {
                    let activation = WarpMap::identity(prepared.warp_map)
                        .reanchor(prepared.source, prepared.activation);
                    Arc::new(plan.with_activation(activation))
                });
            self.runtime.core.items.set_track_plan(item, plan);

            if let Some(slot) = self.runtime.slot() {
                let sample_rate = self.runtime.core.engine.master_sample_rate().max(1);
                let sample_rate = u64::from(sample_rate);
                let target =
                    kithara_platform::time::Duration::from_secs(prepared.source / sample_rate)
                        + kithara_platform::time::Duration::from_nanos(
                            prepared.source % sample_rate * 1_000_000_000 / sample_rate,
                        );
                if let Some(seek) = self
                    .runtime
                    .core
                    .engine
                    .begin_track_seek(slot, item, target)
                    && matches!(seek.outcome, kithara_audio::SeekOutcome::Landed { .. })
                    && let Err(error) =
                        self.runtime
                            .send_to_slot(crate::bridge::PlayerCmd::ScheduleSeek {
                                item_id: item,
                                seek_epoch: seek.epoch,
                            })
                {
                    warn!(%error, %item, seek_epoch = seek.epoch, "scheduled Warp seek command was not admitted");
                }
            }
        }
        Ok(Some(admission))
    }

    /// Acknowledges the prepared warp map once matching PCM reaches presentation.
    ///
    /// # Errors
    ///
    /// Returns the group's acknowledgement error.
    pub(crate) fn acknowledge_prepared(&mut self) -> Result<Option<SyncStatusSnapshot>, SyncError> {
        let sync = &self.sync;
        #[cfg(target_arch = "wasm32")]
        let sync = sync.owned()?;
        let Some(prepared) = sync.prepared() else {
            return Ok(None);
        };
        let frontier = self.runtime.presentation_frontier();
        if !prepared_is_presented(frontier, prepared.activation, prepared.warp_map) {
            return Ok(None);
        }
        let (load, transport) = sync.generations();
        let topology = self.sync.topology()?.stamp();
        let applied = SyncApplied::builder()
            .group(self.sync.snapshot().stamp())
            .load(load)
            .frontier(frontier)
            .operation(prepared.operation)
            .topology(topology)
            .transport(transport)
            .warp_map(prepared.warp_map)
            .build();
        self.sync.acknowledge(applied).map(Some)
    }

    fn replan_track(&self, item: TrackId) {
        let Some(grid) = self.runtime.core.items.track_grid(item) else {
            return;
        };
        let plan = grid
            .segments
            .region_plan()
            .inspect_err(|error| warn!(%error, %item, "track grid has no region plan"))
            .ok();
        self.runtime
            .core
            .items
            .set_track_plan(item, plan.map(Arc::new));
    }

    fn transact_sync(
        &mut self,
        operation: SyncOperation<PlayerMember>,
    ) -> Result<SyncAdmission, SyncError> {
        self.sync.transact(operation).map_err(|rejected| {
            let (error, _): (SyncError, SyncOperation<PlayerMember>) = rejected.into();
            error
        })
    }
}

fn prepared_is_presented(
    frontier: kithara_warp::PresentationFrontier,
    activation: kithara_warp::SessionFrame,
    warp_map: kithara_warp::WarpMapRevision,
) -> bool {
    frontier.output() >= activation && frontier.warp_map() == Some(warp_map)
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;
    use kithara_warp::{PresentationFrontier, SessionFrame, WarpMapRevision};

    use super::prepared_is_presented;

    #[kithara::test]
    fn buffered_pcm_from_the_previous_map_cannot_acknowledge_a_prepared_map() {
        let activation = SessionFrame::new(24_000);
        let revision = WarpMapRevision::first();
        let old_pcm = PresentationFrontier::builder()
            .source(24_000)
            .output(activation)
            .build();
        let applied_pcm = PresentationFrontier::builder()
            .source(24_000)
            .output(activation)
            .warp_map(revision)
            .build();

        assert!(!prepared_is_presented(old_pcm, activation, revision));
        assert!(prepared_is_presented(applied_pcm, activation, revision));
    }
}
