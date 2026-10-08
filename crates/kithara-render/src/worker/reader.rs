use std::{fmt, num::NonZeroUsize};

use kithara_abr::AbrHandle;
use kithara_audio::{Audio, DecodeErrorKind};
use kithara_decode::TrackMetadata;
use kithara_platform::{
    sync::{Arc, ThreadGate, WaitGate},
    time::Duration,
};
use kithara_signal::{AudioChunk, AudioSpec, SegmentId};
use kithara_stream::WorkerWake;
use ringbuf::{
    HeapCons, HeapProd, HeapRb,
    traits::{Consumer, Observer, Producer, Split},
};
use triple_buffer::{Input, Output, triple_buffer};

use super::scheduler::Wake;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::test_pools::{pools, sample_buffer};
    use kithara_signal::{AudioChunkInfo, SourceSpan};

    pub(crate) struct PacketRing {
        pub(crate) receiver: Option<PcmReceiver>,
        producer: PcmProducer,
    }

    impl PacketRing {
        pub(crate) fn new(spec: AudioSpec, duration: Duration, capacity: usize) -> Self {
            let (forward_tx, forward_rx) = HeapRb::new(capacity).split();
            let (reverse_tx, reverse_rx) = HeapRb::new(capacity).split();
            let (playing_tx, playing_rx) = triple_buffer(&false);
            Self {
                receiver: Some(PcmReceiver {
                    forward: forward_rx,
                    reverse: reverse_tx,
                    playing: playing_tx,
                    ready: None,
                    wake: Wake::new(kithara_worker::Wake::default()),
                    spec,
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
                    ready: None,
                },
            }
        }

        pub(in crate::worker) fn into_ends(mut self) -> (PcmReceiver, PcmProducer) {
            (self.receiver.take().expect("receiver"), self.producer)
        }

        pub(crate) fn playing(&mut self) -> bool { *self.producer.playing.read() }

        pub(crate) fn push(&mut self, packet: PcmPacket) {
            self.producer.forward.try_push(packet).expect("packet ring space");
        }

        pub(crate) fn returned(&mut self) -> Option<PcmPacket> {
            self.producer.reverse.try_pop()
        }
    }

    pub(crate) fn chunk(
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
                end_timestamp: spec.duration_for(source_frame + frames).expect("end timestamp"),
                source_span: SourceSpan::new(source_frame, source_frame + frames, spec.sample_rate, frames),
                ..AudioChunkInfo::default()
            },
            sample_buffer(&pools(), samples),
        )
    }
}

/// One owning output packet, including terminal output for a lane segment.
#[derive(Debug)]
pub enum PcmPacket {
    Chunk(AudioChunk),
    Failed {
        segment: SegmentId,
        kind: DecodeErrorKind,
    },
}

pub(super) struct PcmProducer {
    pub(super) forward: HeapProd<PcmPacket>,
    pub(super) reverse: HeapCons<PcmPacket>,
    pub(super) playing: Output<bool>,
    ready: Option<Arc<ThreadGate>>,
}

impl PcmProducer {
    pub(super) fn signal(&self) {
        if let Some(ready) = &self.ready {
            ready.signal();
        }
    }
}

impl Drop for PcmProducer {
    fn drop(&mut self) {
        self.signal();
    }
}

/// Receiver of rendered PCM; every packet is returned to the owning lane.
pub struct PcmReceiver {
    forward: HeapCons<PcmPacket>,
    reverse: HeapProd<PcmPacket>,
    playing: Input<bool>,
    ready: Option<Arc<ThreadGate>>,
    wake: Wake,
    spec: AudioSpec,
    duration: Option<Duration>,
    position: Duration,
    frontier: Duration,
    metadata: TrackMetadata,
    abr: Option<AbrHandle>,
}

impl PcmReceiver {
    pub(super) fn new<T>(
        capacity: NonZeroUsize,
        block_on_underrun: bool,
        wake: Wake,
        audio: &Audio<T>,
        position: Duration,
    ) -> (Self, PcmProducer) {
        let (forward_tx, forward_rx) = HeapRb::new(capacity.get()).split();
        let (reverse_tx, reverse_rx) = HeapRb::new(capacity.get()).split();
        let (playing_tx, playing_rx) = triple_buffer(&false);
        let ready = block_on_underrun.then(|| Arc::new(ThreadGate::default()));
        (
            Self {
                forward: forward_rx,
                reverse: reverse_tx,
                playing: playing_tx,
                ready: ready.clone(),
                wake,
                spec: audio.spec(),
                duration: audio.duration(),
                position,
                frontier: position,
                metadata: audio.metadata().clone(),
                abr: audio.abr_handle(),
            },
            PcmProducer {
                forward: forward_tx,
                reverse: reverse_rx,
                playing: playing_rx,
                ready,
            },
        )
    }

    #[must_use]
    pub const fn spec(&self) -> AudioSpec {
        self.spec
    }

    #[must_use]
    pub const fn duration(&self) -> Option<Duration> {
        self.duration
    }

    #[must_use]
    pub const fn position(&self) -> Duration {
        self.position
    }

    #[must_use]
    pub fn decoded_frontier(&self) -> Duration {
        match self.forward.last() {
            Some(PcmPacket::Chunk(chunk)) => chunk
                .meta
                .source_span
                .and_then(|span| span.position_at(span.output_frames()))
                .unwrap_or(self.frontier),
            _ => self.frontier,
        }
    }

    #[must_use]
    pub fn cached_span(&self) -> Duration {
        self.decoded_frontier().saturating_sub(self.position)
    }

    #[must_use]
    pub const fn metadata(&self) -> &TrackMetadata {
        &self.metadata
    }

    /// Adaptive bitrate control retained from the one source open.
    #[must_use]
    pub fn abr_handle(&self) -> Option<AbrHandle> {
        self.abr.clone()
    }

    /// Publish transport activity through the ring's deferred wake path.
    pub fn set_playing(&mut self, playing: bool) {
        self.playing.write(playing);
        self.notify();
    }

    /// Record the exact consumed source position supplied by the PCM owner.
    pub fn set_position(&mut self, position: Duration) {
        self.position = position;
    }

    #[must_use]
    pub fn peek(&self) -> Option<&PcmPacket> {
        let packet = self.forward.try_peek();
        if packet.is_none() {
            self.notify();
        }
        packet
    }

    /// Try to take one packet without waiting, regardless of blocking policy.
    pub fn pop(&mut self) -> Option<PcmPacket> {
        let packet = self.forward.try_pop();
        if let Some(PcmPacket::Chunk(chunk)) = &packet {
            self.spec = chunk.spec();
            if let Some(position) = chunk
                .meta
                .source_span
                .and_then(|span| span.position_at(span.output_frames()))
            {
                self.frontier = position;
            }
        }
        self.notify();
        packet
    }

    /// Take a packet off-RT, waiting only when blocking reads were configured.
    ///
    /// This method can block and must not be called from the audio callback.
    pub fn pop_blocking(&mut self) -> Option<PcmPacket> {
        loop {
            let since = self.ready.as_ref().map(|ready| ready.current());
            if let Some(packet) = self.pop() {
                return Some(packet);
            }
            let Some((ready, since)) = self.ready.as_ref().zip(since) else {
                return None;
            };
            if !self.forward.write_is_held() {
                return None;
            }
            self.wake.wake();
            ready.wait_timeout(since, crate::consts::ACTIVE_WAIT_TIMEOUT);
        }
    }

    /// Return a packet for off-RT reclamation, retaining it on a full ring.
    ///
    /// # Errors
    /// Returns the original packet when the reverse ring is full.
    pub fn recycle(&mut self, packet: PcmPacket) -> Result<(), PcmPacket> {
        let result = self.reverse.try_push(packet);
        self.notify();
        result
    }

    fn notify(&self) {
        self.wake.defer();
    }
}

impl fmt::Debug for PcmReceiver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PcmReceiver")
            .field("spec", &self.spec)
            .field("duration", &self.duration)
            .field("position", &self.position)
            .finish_non_exhaustive()
    }
}
