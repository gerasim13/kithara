use kithara_decode::{PcmChunk, duration_for_frames};
use kithara_platform::time::Duration;
use kithara_stream::AudioCodec;

use crate::pipeline::seek::skip::frames as frames_for;

/// Where a decoder generation puts frame zero.
///
/// Two things move it. A decoder labels its output by counting frames it
/// emitted, but it drops audio the container still counts, so its count sits
/// behind the container's clock by however much it dropped — and how much that
/// is depends on where the generation started. And the container itself starts
/// ahead of the music: an AAC stream carries encoder priming before the first
/// audible frame, a FLAC stream carries none, so the same instant of music is
/// a different reading on each.
///
/// Which is why nothing downstream may take a generation's labels at face
/// value. They are readings on that generation's own scale, and the pipeline
/// keeps a single scale — the first generation's — that every later one is
/// converted onto ([`Origin::rebase`]), including across a change of codec.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Origin {
    frames: i64,
}

impl Origin {
    /// Read a generation's origin off a chunk it produced and the codec that
    /// produced it: the chunk carries the container's clock as `timestamp` and
    /// the generation's own count as `frame_offset`, and the codec says how far
    /// that container runs ahead of the music.
    pub(crate) fn of(chunk: &PcmChunk, codec: Option<AudioCodec>) -> Self {
        let clock = frames_for(chunk.meta.spec, chunk.meta.timestamp) as i64;
        let counted = i64::try_from(chunk.meta.frame_offset).unwrap_or(i64::MAX);
        Self {
            frames: clock - counted - content_origin(codec),
        }
    }

    /// Move a chunk from the scale it was labelled on onto `anchor`.
    ///
    /// Position labels only; the samples are untouched.
    pub(crate) fn rebase(self, anchor: Self, chunk: &mut PcmChunk) {
        let rate = chunk.meta.spec.sample_rate.get();
        let by = self.frames - anchor.frames;
        if by == 0 {
            return;
        }
        let magnitude = by.unsigned_abs();
        let shift = duration_for_frames(rate, magnitude);
        if by > 0 {
            chunk.meta.frame_offset = chunk.meta.frame_offset.saturating_add(magnitude);
            chunk.meta.timestamp = chunk.meta.timestamp.saturating_add(shift);
            chunk.meta.end_timestamp = chunk.meta.end_timestamp.saturating_add(shift);
        } else {
            chunk.meta.frame_offset = chunk.meta.frame_offset.saturating_sub(magnitude);
            chunk.meta.timestamp = chunk.meta.timestamp.saturating_sub(shift);
            chunk.meta.end_timestamp = chunk.meta.end_timestamp.saturating_sub(shift);
        }
    }
}

/// How far a codec's container runs ahead of the music it carries.
fn content_origin(codec: Option<AudioCodec>) -> i64 {
    codec.map_or(0, |codec| {
        i64::try_from(AudioCodec::encoder_priming_frames(codec)).unwrap_or(0)
    })
}

/// Read a position quoted on `anchor`'s scale as a position on `codec`'s own
/// container clock — what a decoder for that codec has to be asked for.
pub(crate) fn on_container_clock(
    position: Duration,
    anchor: Option<AudioCodec>,
    codec: Option<AudioCodec>,
    sample_rate: u32,
) -> Duration {
    let by = content_origin(codec) - content_origin(anchor);
    let shift = duration_for_frames(sample_rate, by.unsigned_abs());
    if by > 0 {
        position.saturating_add(shift)
    } else {
        position.saturating_sub(shift)
    }
}
