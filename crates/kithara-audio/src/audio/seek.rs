use kithara_events::EventBus;
use kithara_platform::{sync::Arc, time::Duration};
use kithara_stream::{DeferredWake, PlayheadWrite, SeekControl, SeekPrepare};
use tracing::trace;

use super::{PreloadGate, SeekOutcome};
use crate::{AudioEvent, SeekLifecycleStage, SegmentLocation, traits::SeekBegin};

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
    /// The gate is rearmed before the epoch is published, since the worker may complete preload
    /// immediately.
    fn begin(&self, position: Duration) -> SeekOutcome {
        if let Some(prepare) = &self.seek_prepare {
            prepare.prepare();
        }
        self.preload_gate.rearm();
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

#[cfg(test)]
mod tests {
    use kithara_stream::{PlayheadState, SeekState, WorkerWake};
    use kithara_test_utils::kithara;

    use super::*;

    struct ReadyDuringSeek {
        gate: Arc<PreloadGate>,
        state: SeekState,
    }

    impl SeekControl for ReadyDuringSeek {
        fn begin(&self, target: Duration) -> u64 {
            let epoch = self.state.begin(target);
            // The worker can complete preload as soon as the epoch is visible.
            self.gate.rearm();
            self.gate.signal_epoch(epoch);
            epoch
        }

        delegate::delegate! {
            to self.state {
                fn clear_pending(&self, epoch: u64);
                fn complete(&self, epoch: u64);
                fn mark_pending(&self, epoch: u64);
            }
        }
    }

    impl WorkerWake for ReadyDuringSeek {
        fn defer(&self) {}
        fn wake(&self) {}
    }

    #[kithara::test]
    fn seek_preserves_preload_completed_before_begin_returns() {
        let gate = Arc::new(PreloadGate::default());
        gate.signal_epoch(0);
        let source = Arc::new(ReadyDuringSeek {
            state: SeekState::default(),
            gate: Arc::clone(&gate),
        });
        let handle = SeekHandle::new(SeekHandleParts {
            playhead: Arc::new(PlayheadState::default()),
            preload_gate: Arc::clone(&gate),
            seek: source.clone(),
            wake: source,
            bus: EventBus::new(8),
            peer_wake: None,
            seek_prepare: None,
        });

        let _ = handle.begin(Duration::from_secs(1));

        assert!(
            gate.is_ready_for_epoch(1),
            "seek must preserve the worker's readiness signal"
        );
    }
}
