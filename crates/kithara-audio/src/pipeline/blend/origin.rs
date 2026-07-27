use kithara_decode::{PcmChunk, duration_for_frames};

use crate::pipeline::seek::skip::frames as frames_for;

/// Where a decoder generation puts frame zero.
///
/// A decoder labels its output by counting frames it emitted, but it drops
/// audio the container still counts — encoder priming, a codec's own
/// algorithmic delay — so its count sits behind the container's clock by
/// however much it dropped. That distance is this. Two generations of the same
/// track do not have to agree on it: how much a generation drops depends on
/// where it started, and one created mid-track by a variant switch drops a
/// different amount than one that started at the head.
///
/// Which is why nothing downstream may take a generation's labels at face
/// value. They are readings on that generation's own scale, and the pipeline
/// keeps a single scale — the first generation's — that every later one is
/// converted onto ([`Origin::rebase`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Origin {
    frames: u64,
}

impl Origin {
    /// Read a generation's origin off a chunk it produced: the chunk carries
    /// both readings — the container's clock as `timestamp`, the generation's
    /// own count as `frame_offset` — and the gap between them is the answer.
    pub(crate) fn of(chunk: &PcmChunk) -> Self {
        let clock = frames_for(chunk.meta.spec, chunk.meta.timestamp) as u64;
        Self {
            frames: clock.saturating_sub(chunk.meta.frame_offset),
        }
    }

    /// Move a chunk from the scale it was labelled on onto `anchor`.
    ///
    /// Position labels only; the samples are untouched. A generation that
    /// dropped more than the anchor did is reading low, so its labels move up
    /// by the difference, and the other way round.
    pub(crate) fn rebase(self, anchor: Self, chunk: &mut PcmChunk) {
        if self == anchor {
            return;
        }
        let rate = chunk.meta.spec.sample_rate.get();
        if self.frames >= anchor.frames {
            let by = self.frames - anchor.frames;
            chunk.meta.frame_offset = chunk.meta.frame_offset.saturating_add(by);
            let shift = duration_for_frames(rate, by);
            chunk.meta.timestamp = chunk.meta.timestamp.saturating_add(shift);
            chunk.meta.end_timestamp = chunk.meta.end_timestamp.saturating_add(shift);
        } else {
            let by = anchor.frames - self.frames;
            chunk.meta.frame_offset = chunk.meta.frame_offset.saturating_sub(by);
            let shift = duration_for_frames(rate, by);
            chunk.meta.timestamp = chunk.meta.timestamp.saturating_sub(shift);
            chunk.meta.end_timestamp = chunk.meta.end_timestamp.saturating_sub(shift);
        }
    }
}
