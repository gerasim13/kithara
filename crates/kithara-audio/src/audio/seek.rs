use kithara_events::{AudioEvent, EventBus, SeekLifecycleStage, SegmentLocation};
use kithara_platform::{sync::Arc, time::Duration};
use kithara_stream::{DeferredWake, PlayheadWrite, SeekControl, SeekPrepare};
use tracing::trace;

use super::{PreloadGate, SeekOutcome};
use crate::traits::SeekBegin;

/// The control-plane half of a seek: rebuilds the source's byte space, publishes a lifecycle event,
/// nudges the peer and wakes the worker. Each takes a lock, so the audio thread only runs
/// [`Audio::sync_seek`](super::Audio::sync_seek).
pub struct SeekHandle {
    playhead: Arc<dyn PlayheadWrite>,
    preload_gate: Arc<PreloadGate>,
    seek: Arc<dyn SeekControl>,
    wake: Arc<dyn kithara_stream::WorkerWake>,
    bus: EventBus,
    peer_wake: Option<Arc<DeferredWake>>,
    seek_prepare: Option<Arc<dyn SeekPrepare>>,
}

impl SeekHandle {
    pub(super) fn new(parts: SeekHandleParts) -> Self {
        let SeekHandleParts {
            bus,
            peer_wake,
            playhead,
            preload_gate,
            seek,
            seek_prepare,
            wake,
        } = parts;
        Self {
            playhead,
            preload_gate,
            seek,
            wake,
            bus,
            peer_wake,
            seek_prepare,
        }
    }
}

impl SeekBegin for SeekHandle {
    fn begin(&self, position: Duration) -> SeekOutcome {
        if let Some(prepare) = &self.seek_prepare {
            prepare.prepare();
        }
        let epoch = self.seek.begin(position);
        self.seek.mark_pending(epoch);
        self.bus.publish(AudioEvent::SeekLifecycle {
            seek_epoch: epoch,
            stage: SeekLifecycleStage::SeekRequest,
            location: SegmentLocation::default(),
        });
        if let Some(wake) = &self.peer_wake {
            wake.notify_now();
        }
        self.preload_gate.rearm();
        self.wake.wake();

        trace!(?position, epoch, "seek begun");
        match self.playhead.duration() {
            Some(duration) if position >= duration => SeekOutcome::PastEof {
                duration,
                target: position,
            },
            _ => SeekOutcome::Landed {
                target: position,
                landed_at: position,
            },
        }
    }
}

pub(super) struct SeekHandleParts {
    pub(super) playhead: Arc<dyn PlayheadWrite>,
    pub(super) preload_gate: Arc<PreloadGate>,
    pub(super) seek: Arc<dyn SeekControl>,
    pub(super) wake: Arc<dyn kithara_stream::WorkerWake>,
    pub(super) bus: EventBus,
    pub(super) peer_wake: Option<Arc<DeferredWake>>,
    pub(super) seek_prepare: Option<Arc<dyn SeekPrepare>>,
}
