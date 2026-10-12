use kithara_decode::TrackMetadata;
use kithara_platform::{
    sync::{Arc, ThreadGate, WaitGate},
    time::Duration,
};
use kithara_signal::{AudioChunk, AudioChunkInfo, AudioSpec, SegmentId, SourceSpan};
use kithara_stream::WorkerWake;
use kithara_test_utils::{
    bufpool::{pools, sample_buffer},
    kithara,
};
use kithara_worker::{DispatcherConfig, Wake, Worker, WorkerConfig};
use ringbuf::{
    HeapRb,
    traits::{Consumer, Observer, Producer, Split},
};
use triple_buffer::triple_buffer;

use super::{
    PcmPacket, PcmReceiver,
    core::{FinalWake, PcmProducer},
};
use crate::worker::scheduler::StreamWake;

pub struct PacketRing {
    pub receiver: Option<PcmReceiver>,
    pub(in crate::worker) producer: PcmProducer,
}

impl PacketRing {
    #[must_use]
    pub fn new(spec: AudioSpec, duration: Duration, capacity: usize) -> Self {
        Self::with_wake(
            spec,
            duration,
            capacity,
            capacity,
            false,
            StreamWake::new(Wake::default()),
        )
    }

    pub(in crate::worker) fn with_wake(
        spec: AudioSpec,
        duration: Duration,
        capacity: usize,
        reverse_capacity: usize,
        blocking: bool,
        wake: StreamWake,
    ) -> Self {
        let (forward_tx, forward_rx) = HeapRb::new(capacity).split();
        let (reverse_tx, reverse_rx) = HeapRb::new(reverse_capacity).split();
        let (playing_tx, playing_rx) = triple_buffer(&false);
        let ready = blocking.then(|| Arc::new(ThreadGate::default()));
        Self {
            receiver: Some(PcmReceiver {
                render_publisher: None,
                spec,
                forward: forward_rx,
                reverse: reverse_tx,
                playing: playing_tx,
                ready: ready.clone(),
                wake,
                duration: Some(duration),
                position: Duration::ZERO,
                frontier: Duration::ZERO,
                metadata: TrackMetadata::default(),
                abr: None,
            }),
            producer: PcmProducer {
                forward: forward_tx,
                reverse: reverse_rx,
                playing: playing_rx,
                ready: FinalWake(ready),
                events: kithara_events::EventBus::default(),
                output_pending: false,
            },
        }
    }

    pub(in crate::worker) fn into_ends(mut self) -> (PcmReceiver, PcmProducer) {
        (self.receiver.take().expect("receiver"), self.producer)
    }

    pub fn playing(&mut self) -> bool {
        *self.producer.playing.read()
    }

    /// Places one packet in the fixture's forward ring.
    ///
    /// # Panics
    /// Panics if the forward ring is full.
    pub fn push(&mut self, packet: PcmPacket) {
        self.producer
            .forward
            .try_push(packet)
            .expect("packet ring space");
    }

    pub fn returned(&mut self) -> Option<PcmPacket> {
        self.producer.reverse.try_pop()
    }
}

/// Copies samples into a chunk with the specified lane and source positions.
///
/// # Panics
/// Panics for invalid frame geometry, unrepresentable timestamps, or exhausted pools.
#[must_use]
pub fn chunk(
    spec: AudioSpec,
    segment: SegmentId,
    lane_frame: u64,
    source_frame: u64,
    samples: &[f32],
) -> AudioChunk {
    let frames = u64::try_from(samples.len() / usize::from(spec.channels)).expect("frames");
    AudioChunk::new(
        AudioChunkInfo {
            spec,
            frames: u32::try_from(frames).expect("chunk frames"),
            segment,
            lane_frame,
            frame_offset: source_frame,
            timestamp: spec.duration_for(source_frame).expect("timestamp"),
            end_timestamp: spec
                .duration_for(source_frame + frames)
                .expect("end timestamp"),
            source_span: if frames == 0 {
                SourceSpan::new(source_frame, source_frame + 1, spec.sample_rate, 1)
                    .and_then(|span| span.for_output_range(0..0))
            } else {
                SourceSpan::new(
                    source_frame,
                    source_frame + frames,
                    spec.sample_rate,
                    frames,
                )
            },
            ..AudioChunkInfo::default()
        },
        sample_buffer(&pools(), samples),
    )
}

pub(in crate::worker) fn packet_fixture(
    blocking: bool,
    spec: AudioSpec,
) -> (PcmReceiver, PcmProducer) {
    let (forward, received) = HeapRb::new(8).split();
    let (returned, reverse) = HeapRb::new(8).split();
    let (playing, activity) = triple_buffer(&false);
    let ready = blocking.then(|| Arc::new(ThreadGate::default()));
    let worker = Worker::new(WorkerConfig::new());
    let dispatcher =
        worker.dispatcher(DispatcherConfig::builder().name("terminal-fixture").build());
    (
        PcmReceiver {
            render_publisher: None,
            playing,
            spec,
            forward: received,
            reverse: returned,
            ready: ready.clone(),
            wake: StreamWake::new(dispatcher.wake_handle()),
            duration: None,
            position: Duration::ZERO,
            frontier: Duration::ZERO,
            metadata: TrackMetadata::default(),
            abr: None,
        },
        PcmProducer {
            forward,
            reverse,
            playing: activity,
            ready: FinalWake(ready),
            events: kithara_events::EventBus::default(),
            output_pending: false,
        },
    )
}

/// Waits for fixture PCM or producer closure with a one-second liveness budget.
#[kithara::hang_watchdog(timeout = Duration::from_secs(1))]
pub fn wait_for_packet(receiver: &PcmReceiver) {
    let Some(ready) = &receiver.ready else {
        return;
    };
    loop {
        hang_tick!();
        let since = ready.current();
        if receiver.forward.try_peek().is_some() || !receiver.forward.write_is_held() {
            hang_reset!();
            return;
        }
        receiver.wake.wake();
        hang_park!(|remaining: Duration| {
            ready.wait_timeout(since, remaining);
        });
    }
}
