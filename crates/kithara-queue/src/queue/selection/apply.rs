use std::sync::PoisonError;

use kithara_events::{AdvanceReason, QueueEvent, TrackId, TrackStatus};
use kithara_platform::{sync::Arc, tokio::task};
use kithara_play::{Resource, SelectTransition};
use tracing::{debug, warn};

use crate::{
    error::QueueError,
    queue::{QueueControl, types::SelectPhase},
};

impl QueueControl {
    pub(super) fn watch_apply(
        &self,
        id: TrackId,
        handle: Option<task::JoinHandle<Result<Resource, QueueError>>>,
    ) {
        if self.is_closed() {
            return;
        }
        let Some(handle) = handle else {
            return;
        };
        let player = self.player.clone();
        let tracks = Arc::clone(&self.tracks);
        let pending_select = Arc::clone(&self.pending_select);
        let navigation = Arc::clone(&self.navigation);
        let select_apply = Arc::clone(&self.select_apply);
        let bus = self.bus.clone();
        let queue = self.clone();
        drop(task::spawn(async move {
            let resource = match handle.await {
                Ok(Ok(resource)) => resource,
                Ok(Err(_)) => return,
                Err(join_err) => {
                    warn!(id = id.as_u64(), error = %join_err, "loader join failed");
                    return;
                }
            };

            let _admission = queue.lock_admission();
            if queue.is_closed() {
                return;
            }

            // Held across the whole synchronous block (never across .await):
            // the Cancelled re-check and select_item must be atomic w.r.t. a
            let _apply = select_apply.lock().unwrap_or_else(PoisonError::into_inner);

            if player.is_closed() {
                return;
            }

            let was_cancelled = tracks
                .lock()
                .iter()
                .find(|entry| entry.id == id)
                .is_some_and(|entry| matches!(entry.status, TrackStatus::Cancelled));
            if was_cancelled {
                debug!(
                    id = id.as_u64(),
                    "load was overridden by a later select; skipping replace_item"
                );
                return;
            }

            let index = {
                let guard = tracks.lock();
                guard.iter().position(|entry| entry.id == id)
            };
            let Some(index) = index else {
                debug!(
                    id = id.as_u64(),
                    "load completed but track no longer in queue"
                );
                return;
            };

            if let Err(error) = player.replace_item(index, resource) {
                debug!(id = id.as_u64(), %error, "player closed before load could be applied");
                return;
            }
            tracks.set_status(id, TrackStatus::Loaded);
            if tracks.lock().get(index).is_some_and(|entry| entry.id == id) {
                bus.publish(QueueEvent::NextTrackReady { id, index });
            }

            let pending_transition = {
                let mut phase = pending_select
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                let result = match *phase {
                    SelectPhase::Pending(pending) if pending.id == id => {
                        *phase = SelectPhase::Idle;
                        Some(pending.transition)
                    }
                    _ => None,
                };
                drop(phase);
                result
            };
            let mark_consumed = || {
                tracks.set_status(id, TrackStatus::Consumed);
            };

            if let Some(transition) = pending_transition {
                let was_playing = player.is_playing();
                let crossfade = transition.crossfade_seconds(player.crossfade_duration());
                if was_playing && crossfade > 0.0 {
                    bus.publish(QueueEvent::CrossfadeStarted {
                        duration_seconds: crossfade,
                    });
                }
                if let Err(error) = player.select_item_with_crossfade(
                    index,
                    SelectTransition {
                        autoplay: true,
                        crossfade_seconds: crossfade,
                    },
                ) {
                    warn!(id = id.as_u64(), error = %error, "pending select failed");
                } else {
                    navigation
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .select(index);
                    bus.publish(QueueEvent::CurrentTrackAdvance {
                        id: Some(id),
                        reason: AdvanceReason::UserSelect,
                    });
                    mark_consumed();
                }
            }
        }));
    }
}
