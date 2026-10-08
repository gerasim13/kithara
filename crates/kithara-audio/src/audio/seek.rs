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
