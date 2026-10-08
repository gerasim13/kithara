#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use kithara_platform::{CancelToken, sync::Arc};
    use kithara_signal::{AudioChunk, AudioChunkInfo};
    use kithara_stream::PlayheadState;
    use kithara_test_fixtures::mock_fixtures::ring_pcm;
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        ConsumerWakeMode, TrackFailureKind,
        audio::ReadOutcome,
        test_pools::{Pools, pools, sample_buffer},
    };

    struct RingFixture {
        playhead: Arc<PlayheadState>,
        events: crate::audio::event::AudioEvents,
        cursor: ChunkCursor,
        _trash_rx: Inlet<AudioChunk>,
        data_tx: Outlet<Fetch<AudioChunk>>,
        pools: Pools,
        ring: RingConsumer,
    }

    impl RingFixture {
        fn new(preloaded: bool) -> Self {
            Self::with_wake_mode(preloaded, false, ConsumerWakeMode::RealtimeDeferred)
        }

        fn chunk(&self, samples: &[f32]) -> AudioChunk {
            let mut meta = AudioChunkInfo::default();
            meta.spec.channels = 1;
            meta.frames = u32::try_from(samples.len()).unwrap_or(u32::MAX);
            AudioChunk::new(meta, sample_buffer(&self.pools, samples))
        }

        fn recv(&mut self) -> Option<AudioChunk> {
            self.ring
                .recv_valid_chunk(empty_ctx(), Wait::ForProducer)
                .map(|(chunk, _source_span)| chunk)
        }

        fn with_wake_mode(
            preloaded: bool,
            block_on_underrun: bool,
            consumer_wake_mode: ConsumerWakeMode,
        ) -> Self {
            let pools = pools();
            let (data_tx, audio_rx) = connect::<Fetch<AudioChunk>>(4, None);
            let (trash_tx, trash_rx) = connect::<AudioChunk>(8, None);
            let mut ring = RingConsumer::new(RingParts {
                audio_rx,
                trash_tx,
                block_on_underrun,
                consumer_wake_mode,
                reader_wake: Arc::new(ThreadWake::default()),
                epoch: Arc::new(AtomicU64::new(0)),
            });
            ring.preloaded = preloaded;
            Self {
                pools,
                ring,
                data_tx,
                cursor: ChunkCursor::new(AudioChunkInfo::default().spec),
                events: crate::audio::event::AudioEvents::test(),
                playhead: Arc::new(PlayheadState::new()),
                _trash_rx: trash_rx,
            }
        }
    }

    fn empty_ctx() -> RecvCtx<'static> {
        RecvCtx {
            cancel: None,
            worker: None,
            abr: None,
        }
    }

    /// Counts every statement of demand the ring makes, whichever wake mode
    /// carries it.
    #[derive(Default)]
    struct DemandCounter {
        stated: AtomicU64,
    }

    impl WorkerWake for DemandCounter {
        fn defer(&self) {
            self.stated.fetch_add(1, Ordering::Release);
        }

        fn wake(&self) {
            self.stated.fetch_add(1, Ordering::Release);
        }
    }



    #[kithara::test]
    fn a_nonblocking_poll_of_an_empty_ring_states_its_demand() {
        let mut fixture = RingFixture::new(true);
        let worker = DemandCounter::default();

        let outcome = fixture.ring.recv_outcome(
            RecvCtx {
                cancel: None,
                worker: Some(&worker),
                abr: None,
            },
            Wait::ForProducer,
        );

        assert!(matches!(outcome, RecvOutcome::Empty));
        assert_eq!(
            worker.stated.load(Ordering::Acquire),
            1,
            "the producer parks once it reports backpressure, so an empty poll \
             that states no demand leaves nobody to fill the ring"
        );
    }

    #[kithara::test]
    fn block_on_underrun_forces_immediate_off_rt_wakes() {
        let fixture = RingFixture::with_wake_mode(false, true, ConsumerWakeMode::RealtimeDeferred);

        assert_eq!(
            fixture.ring.consumer_wake_mode,
            ConsumerWakeMode::ImmediateOffRt
        );
    }

    #[kithara::test]
    fn an_adopted_mode_still_yields_to_blocking_reads() {
        let mut fixture =
            RingFixture::with_wake_mode(false, true, ConsumerWakeMode::ImmediateOffRt);

        fixture
            .ring
            .set_consumer_wake_mode(ConsumerWakeMode::RealtimeDeferred);

        assert_eq!(
            fixture.ring.consumer_wake_mode,
            ConsumerWakeMode::ImmediateOffRt
        );
    }

    #[kithara::test]
    fn explicit_off_rt_mode_is_immediate_without_blocking_reads() {
        let fixture = RingFixture::with_wake_mode(true, false, ConsumerWakeMode::ImmediateOffRt);

        assert_eq!(
            fixture.ring.consumer_wake_mode,
            ConsumerWakeMode::ImmediateOffRt
        );
    }

    #[kithara::test]
    fn seek_drain_reports_whether_it_popped_any_item(ring_pcm: Vec<f32>) {
        let mut drained = RingFixture::new(true);
        let first = drained.chunk(&ring_pcm[..1]);
        drained
            .data_tx
            .try_push(Fetch::data(first, 0))
            .expect("first stale chunk reaches ring");
        let second = drained.chunk(&ring_pcm[1..2]);
        drained
            .data_tx
            .try_push(Fetch::data(second, 0))
            .expect("second stale chunk reaches ring");
        drained
            .data_tx
            .try_push(Fetch::eof(1))
            .expect("current epoch marker reaches ring");

        assert!(drained.ring.begin_seek_epoch(1, &mut drained.cursor));

        let mut empty = RingFixture::new(true);
        assert!(!empty.ring.begin_seek_epoch(1, &mut empty.cursor));
    }

    /// One second instead of the ambient ten: the watchdog park is the point of
    /// this test, and on the flash-off lane that park is spent in real time
    /// (measured: 10.087 s ambient vs 0.074 s under flash, where it is virtual).
    #[cfg(not(target_arch = "wasm32"))]
    #[kithara::test(hang_timeout_secs(1))]
    #[should_panic(expected = "recv_outcome_blocking")]
    fn blocking_recv_without_preload_panics_when_no_chunk_arrives() {
        let mut fixture = RingFixture::new(false);
        let _chunk = fixture.recv();
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[kithara::test]
    fn blocking_recv_returns_closed_after_cancel() {
        let mut fixture = RingFixture::new(false);
        let cancel = CancelToken::never();
        cancel.cancel();
        assert!(matches!(
            fixture.ring.recv_outcome(
                RecvCtx {
                    cancel: Some(&cancel),
                    worker: None,
                    abr: None,
                },
                Wait::ForProducer,
            ),
            RecvOutcome::Closed
        ));
    }

    /// The startup latch also opens on an upstream park, so the prime it
    /// authorises has nothing to wait for. One second instead of the ambient
    /// ten for the same reason as
    /// `blocking_recv_without_preload_panics_when_no_chunk_arrives`.
    #[cfg(not(target_arch = "wasm32"))]
    #[kithara::test(hang_timeout_secs(1))]
    fn a_prime_on_an_empty_ring_returns_instead_of_parking() {
        let mut fixture =
            RingFixture::with_wake_mode(true, true, ConsumerWakeMode::RealtimeDeferred);

        assert!(
            !fixture
                .ring
                .fill(&mut fixture.cursor, empty_ctx(), Wait::Never)
        );
    }

    #[kithara::test]
    fn a_prime_takes_a_delivered_chunk(ring_pcm: Vec<f32>) {
        let mut fixture =
            RingFixture::with_wake_mode(true, true, ConsumerWakeMode::RealtimeDeferred);
        let chunk = fixture.chunk(&ring_pcm[..2]);
        fixture
            .data_tx
            .try_push(Fetch::data(chunk, 0))
            .expect("chunk reaches ring");

        assert!(
            fixture
                .ring
                .fill(&mut fixture.cursor, empty_ctx(), Wait::Never)
        );
    }

    /// The prime stopped parking; an audio-thread read under `block_on_underrun`
    /// must still park, which is what trades an underrun for waiting on decode.
    #[cfg(not(target_arch = "wasm32"))]
    #[kithara::test(hang_timeout_secs(1))]
    #[should_panic(expected = "recv_outcome_blocking")]
    fn a_preloaded_read_still_parks_when_underruns_block() {
        let mut fixture =
            RingFixture::with_wake_mode(true, true, ConsumerWakeMode::RealtimeDeferred);

        let _filled = fixture
            .ring
            .fill(&mut fixture.cursor, empty_ctx(), Wait::ForProducer);
    }

    #[kithara::test]
    fn preloaded_recv_is_nonblocking() {
        let mut fixture = RingFixture::new(true);
        assert!(matches!(
            fixture.ring.recv_outcome(empty_ctx(), Wait::ForProducer),
            RecvOutcome::Empty
        ));
    }

    #[kithara::test]
    fn consumer_phase_starts_buffering() {
        let fixture = RingFixture::new(true);
        assert_eq!(fixture.ring.phase, ConsumerPhase::Buffering);
    }

    #[kithara::test]
    fn consumer_phase_transitions_to_playing_on_first_chunk(ring_pcm: Vec<f32>) {
        let mut fixture = RingFixture::new(true);
        let chunk = fixture.chunk(&ring_pcm[..2]);
        fixture
            .data_tx
            .try_push(Fetch::data(chunk, 0))
            .expect("chunk reaches ring");
        assert!(
            fixture
                .ring
                .fill(&mut fixture.cursor, empty_ctx(), Wait::ForProducer)
        );
        assert_eq!(fixture.ring.phase, ConsumerPhase::Playing);
    }

    #[kithara::test]
    fn consumer_phase_transitions_to_seek_pending() {
        let mut fixture = RingFixture::new(true);
        let _ = fixture.ring.begin_seek_epoch(1, &mut fixture.cursor);
        assert!(matches!(
            fixture.ring.phase,
            ConsumerPhase::SeekPending { .. }
        ));
    }

    #[kithara::test]
    fn consumer_phase_seek_pending_to_playing_on_chunk(ring_pcm: Vec<f32>) {
        let mut fixture = RingFixture::new(true);
        let _ = fixture.ring.begin_seek_epoch(1, &mut fixture.cursor);
        let chunk = fixture.chunk(&ring_pcm[..2]);
        fixture
            .data_tx
            .try_push(Fetch::data(chunk, 1))
            .expect("post-seek chunk reaches ring");
        assert!(
            fixture
                .ring
                .fill(&mut fixture.cursor, empty_ctx(), Wait::ForProducer)
        );
        assert_eq!(fixture.ring.phase, ConsumerPhase::Playing);
    }

    #[kithara::test]
    fn seek_drain_preserves_new_epoch_chunk_after_stale_chunks(ring_pcm: Vec<f32>) {
        let mut fixture = RingFixture::new(true);
        let stale = fixture.chunk(&ring_pcm[..2]);
        fixture
            .data_tx
            .try_push(Fetch::data(stale, 0))
            .expect("stale chunk reaches ring");
        let fresh = fixture.chunk(&ring_pcm[2..]);
        fixture
            .data_tx
            .try_push(Fetch::data(fresh, 1))
            .expect("fresh chunk reaches ring");
        let _ = fixture.ring.begin_seek_epoch(1, &mut fixture.cursor);
        let mut buf = [0.0; 2];
        let read = fixture
            .cursor
            .read(
                &mut fixture.ring,
                &mut fixture.events,
                fixture.playhead.as_ref(),
                empty_ctx(),
                &mut buf,
            )
            .expect("post-seek read succeeds");
        let ReadOutcome::Frames { count, .. } = read.outcome else {
            panic!("expected preserved post-seek frames");
        };
        assert_eq!(count.get(), 2);
        assert_eq!(buf, [0.7, 0.8]);
    }

    #[kithara::test]
    fn seek_drain_preserves_new_epoch_eof_after_stale_chunks(ring_pcm: Vec<f32>) {
        let mut fixture = RingFixture::new(true);
        let stale = fixture.chunk(&ring_pcm[..2]);
        fixture
            .data_tx
            .try_push(Fetch::data(stale, 0))
            .expect("stale chunk reaches ring");
        fixture
            .data_tx
            .try_push(Fetch::eof(1))
            .expect("eof reaches ring");
        let _ = fixture.ring.begin_seek_epoch(1, &mut fixture.cursor);
        let mut buf = [0.0; 2];
        let read = fixture
            .cursor
            .read(
                &mut fixture.ring,
                &mut fixture.events,
                fixture.playhead.as_ref(),
                empty_ctx(),
                &mut buf,
            )
            .expect("post-seek eof read succeeds");
        assert!(matches!(read.outcome, ReadOutcome::Eof { .. }));
        assert_eq!(fixture.ring.phase, ConsumerPhase::AtEof);
    }

    #[kithara::test]
    fn consumer_phase_eof_terminates() {
        let mut fixture = RingFixture::new(true);
        fixture
            .data_tx
            .try_push(Fetch::eof(0))
            .expect("eof reaches ring");
        assert!(fixture.recv().is_none());
        assert_eq!(fixture.ring.phase, ConsumerPhase::AtEof);
    }

    #[kithara::test]
    fn consumer_phase_failed_on_channel_close() {
        let mut fixture = RingFixture::new(false);
        let cancel = CancelToken::never();
        cancel.cancel();
        assert!(
            fixture
                .ring
                .recv_valid_chunk(
                    RecvCtx {
                        cancel: Some(&cancel),
                        worker: None,
                        abr: None,
                    },
                    Wait::ForProducer,
                )
                .is_none()
        );
        assert_eq!(
            fixture.ring.phase,
            ConsumerPhase::Failed {
                source: FailureSource::ChannelClosed
            }
        );
    }

    #[kithara::test]
    fn consumer_does_not_park_in_terminal_phase() {
        let mut fixture = RingFixture::new(false);
        fixture.ring.phase = ConsumerPhase::AtEof;
        assert!(fixture.recv().is_none());
    }



    #[kithara::test]
    fn a_stale_natural_eof_does_not_terminate_a_new_seek_epoch() {
        let mut fixture = RingFixture::new(true);
        fixture
            .data_tx
            .try_push(Fetch::eof(0))
            .expect("natural eof reaches ring");

        let _ = fixture.ring.begin_seek_epoch(1, &mut fixture.cursor);

        assert_eq!(fixture.ring.phase, ConsumerPhase::SeekPending { epoch: 1 });
    }

}
