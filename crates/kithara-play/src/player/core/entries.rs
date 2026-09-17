use std::num::NonZeroU32;

use kithara_warp::{AlignmentSource, MapAxis, ReconcileCause, SyncAdmission, SyncRejected};
use num_traits::ToPrimitive;

use super::PlayerImpl;
use crate::{
    api::TrackId,
    player::{
        core::grids::{native_frame, source_cue_beat},
        protocol::PlayerMember,
    },
    sync::EntryRefusal,
};

impl<S> PlayerImpl<S>
where
    S: Send + Sync + 'static,
{
    /// Prepares the entry of every queued track the deck is not yet playing.
    ///
    /// The audible track's own stream says where the crossfade begins, and the
    /// deck carries that frame onto the owner axis as the deadline a waiting
    /// track must enter by. A track that already holds a prepared map keeps it:
    /// the deadline does not move while the same track stays audible.
    pub(crate) fn prepare_pending_entries(&mut self) {
        let Some(current) = self.runtime.core.items.current_item_id() else {
            return;
        };
        let Some(audible) = self.runtime.core.items.track_grid(current) else {
            return;
        };
        if !self.has_queued_grid(current) {
            return;
        }
        let Some(snapshot) = self.runtime.playback_snapshot() else {
            return;
        };
        let axis = audible.snapshot.axis();
        let output_rate = self.runtime.core.engine.output_sample_rate();
        let observed = self.runtime.presentation_frontier();
        let frontier = observed.on_axis(axis, output_rate);
        let source = AlignmentSource::Audible {
            presentation: frontier,
            preparation_source: native_frame(
                snapshot.preparation_source(
                    observed.source(),
                    self.runtime.core.response_budget_frames,
                ),
                axis,
                output_rate,
            ),
            playback_rate: kithara_warp::RateTarget::default().with_speed(snapshot.rate),
        };
        let Some(fade_source) = crossfade_source(
            snapshot.duration(),
            f64::from(self.runtime.crossfade_duration()),
            axis,
            output_rate,
        ) else {
            return;
        };
        let Some(window) = self.sync.entry_window(audible.id, fade_source, source) else {
            return;
        };
        for index in 0..self.runtime.core.items.item_count() {
            let Some(item) = self.runtime.core.items.item_id(index) else {
                continue;
            };
            if item == current {
                continue;
            }
            let Some(grid) = self.runtime.core.items.track_grid(item) else {
                continue;
            };
            self.sync.discard_stale_entry(grid.id, window);
            if self.sync.prepared().get(grid.id).is_some() {
                continue;
            }
            let cue = self
                .runtime
                .core
                .items
                .initial_source_cue(item)
                .and_then(|cue| source_cue_beat(&grid.snapshot, cue));
            if let Err(EntryRefusal::Geometry(required)) =
                self.sync.prepare_entry(grid.id, window, cue)
            {
                tracing::debug!(?required, member = %grid.id, "queued track has no entry geometry");
            }
        }
    }

    pub(crate) fn reconcile_current_grid(
        &mut self,
        cause: ReconcileCause,
        source: Option<AlignmentSource>,
        prepared_launch: bool,
    ) -> Result<Option<SyncAdmission>, SyncRejected<PlayerMember>> {
        let Some(item) = self.runtime.core.items.current_item_id() else {
            return Ok(None);
        };
        if let Some(grid) = self.runtime.core.items.track_grid(item) {
            self.sync.arm_audible(grid.id);
        }
        self.reconcile_item_grid(item, cause, source, prepared_launch)
    }

    /// Whether any queued track other than the audible one carries a grid.
    fn has_queued_grid(&self, current: TrackId) -> bool {
        (0..self.runtime.core.items.item_count()).any(|index| {
            self.runtime.core.items.item_id(index).is_some_and(|item| {
                item != current && self.runtime.core.items.track_grid(item).is_some()
            })
        })
    }
}

/// The frame of the audible stream where its crossfade begins.
///
/// Both durations are media seconds, so carrying them onto the stream's own
/// axis gives its source frame regardless of the rate the deck plays at.
fn crossfade_source(
    duration_seconds: f64,
    fade_seconds: f64,
    axis: MapAxis,
    output_rate: NonZeroU32,
) -> Option<u64> {
    let audible = duration_seconds - fade_seconds.max(0.0);
    if !audible.is_finite() || audible <= 0.0 {
        return None;
    }
    let output_frames = (audible * f64::from(output_rate.get())).round().to_u64()?;
    Some(native_frame(output_frames, axis, output_rate))
}
