//! Owning PCM packets and their nonblocking transport receiver.

use std::fmt;

use kithara_audio::TrackFailureKind;
use kithara_platform::{
    sync::{ThreadGate, Weak},
    time::Duration,
};
use kithara_signal::{AudioChunk, AudioChunkInfo, AudioSpec, SegmentId};
use kithara_worker::Wake;
use ringbuf::{
    HeapCons, HeapProd,
    traits::{Consumer, Observer, Producer},
};
use triple_buffer::Input;

/// One owning output packet, including terminal output for a lane segment.
#[derive(Debug)]
pub enum PcmPacket {
    Chunk(Box<AudioChunk>),
    Failed {
        segment: SegmentId,
        failure: TrackFailureKind,
    },
}

/// Receiver of rendered PCM; every packet is returned to the owning lane.
pub struct PcmReceiver {
    pub(crate) forward: HeapCons<PcmPacket>,
    pub(crate) reverse: HeapProd<PcmPacket>,
    pub(crate) playing: Input<bool>,
    // The producer and active off-RT waiters own the signal.
    pub(crate) ready: Option<Weak<ThreadGate>>,
    pub(crate) wake: Wake,
    pub(crate) spec: AudioSpec,
    pub(crate) frontier: Duration,
}

impl PcmReceiver {
    #[must_use]
    pub const fn spec(&self) -> AudioSpec {
        self.spec
    }

    delegate::delegate! {
        to self {
            /// Raw coordinate of track-media zero from the next immutable PCM packet.
            #[must_use]
            #[expr($?.media_origin)]
            #[call(next_chunk_info)]
            pub fn media_origin(&self) -> Option<Duration>;
            /// Original visible source position published by the decoded-source owner.
            #[must_use]
            #[expr($?.visible_origin)]
            #[call(next_chunk_info)]
            pub fn visible_origin(&self) -> Option<Duration>;
        }
    }

    /// Immutable metadata of the next admitted producer packet.
    #[must_use]
    pub fn next_chunk_info(&self) -> Option<AudioChunkInfo> {
        match self.forward.try_peek()? {
            PcmPacket::Chunk(chunk) => Some(chunk.meta),
            PcmPacket::Failed { .. } => None,
        }
    }

    pub(crate) fn terminal_info(&self) -> Option<AudioChunkInfo> {
        let PcmPacket::Chunk(chunk) = self.forward.last()? else {
            return None;
        };
        chunk.meta.end_of_track.then_some(chunk.meta)
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

    /// Publish transport activity through the ring's deferred wake path.
    pub fn set_playing(&mut self, playing: bool) {
        self.playing.write(playing);
        self.wake.defer();
    }

    #[must_use]
    pub fn peek(&self) -> Option<&PcmPacket> {
        let packet = self.forward.try_peek();
        if packet.is_none() {
            self.wake.defer();
        }
        packet
    }

    pub(crate) fn waits_offline(&self) -> bool {
        self.ready.is_some()
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
        self.wake.defer();
        packet
    }

    #[must_use]
    pub fn is_closed(&self) -> bool {
        !self.forward.write_is_held() && self.forward.try_peek().is_none()
    }

    /// Return a packet for off-RT reclamation, retaining it on a full ring.
    ///
    /// # Errors
    /// Returns the original packet when the reverse ring is full.
    pub fn recycle(&mut self, packet: PcmPacket) -> Result<(), PcmPacket> {
        let result = self.reverse.try_push(packet);
        self.wake.defer();
        result
    }
}

impl fmt::Debug for PcmReceiver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PcmReceiver")
            .field("spec", &self.spec)
            .finish_non_exhaustive()
    }
}
