use kithara_events::TrackId;
use kithara_play::{BeatQuantum, BeatStart, PlayerImpl, TrackAnalysis, TrackBeat};
use tracing::warn;

use super::Queue;
use crate::{attempts::LoadClass, error::QueueError};

impl Queue {
    /// Places this deck on the session grid and starts the item at `index` on
    /// the next beat that lands on `quantum`.
    ///
    /// A running current item transitions its resident tempo stage in place.
    /// A cold item is loaded and armed on the requested beat because it is not
    /// yet in the processor; a stamp sent before that load would be dropped.
    ///
    /// # Errors
    ///
    /// Returns [`QueueError`] when `index` names no track, and forwards
    /// [`PlayError`] when the session has committed no grid or the analysis
    /// carries no usable map.
    pub fn start_at_beat(
        &self,
        index: usize,
        analysis: &TrackAnalysis,
        track_anchor: TrackBeat,
        quantum: BeatQuantum,
    ) -> Result<(), QueueError> {
        let found = {
            let guard = self.lock_tracks();
            let found = guard
                .get(index)
                .map(|entry| (entry.id, entry.source.clone()));
            drop(guard);
            found
        };
        let (id, source) =
            found.ok_or_else(|| QueueError::InvalidUrl(format!("no track at index {index}")))?;
        if self.current_index() == Some(index) && self.player.rate() > 0.0 {
            self.player
                .bind_playing_to_grid(analysis)
                .map_err(QueueError::from)?;
            return Ok(());
        }
        let start = self
            .player
            .bind_to_grid(analysis, track_anchor, quantum)
            .map_err(QueueError::from)?;
        self.set_pending_beat_start(id, start);
        self.spawn_apply_after_load(id, source, LoadClass::Interactive);
        Ok(())
    }

    /// Returns the running deck to free tempo without replacing its resource.
    ///
    /// # Errors
    ///
    /// Returns [`QueueError`] when the deck cannot be released from the grid.
    pub fn unbind_from_grid(&self) -> Result<(), QueueError> {
        self.player.unbind_from_grid().map_err(QueueError::from)
    }

    pub(super) fn set_pending_beat_start(&self, id: TrackId, start: BeatStart) {
        *self.lock_pending_beat_start() = Some((id, start));
    }
}

/// Arms the freshly built resource and stamps its start.
///
/// Called from the load-landing path, where the resource is already in the
/// player's item slot. Arming is what moves it into the processor as
/// preloading, and only a preloading track can be started on a stamped beat.
pub(super) fn arm_landed_beat_start(
    player: &PlayerImpl,
    index: usize,
    id: TrackId,
    start: BeatStart,
) {
    if let Err(error) = player.arm_at_beat(index, start) {
        warn!(id = id.as_u64(), %error, "the beat-anchored start could not be armed");
    }
}
