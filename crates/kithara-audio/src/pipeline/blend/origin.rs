use kithara_decode::{PcmChunk, duration_for_frames};
use kithara_platform::time::Duration;
use kithara_stream::AudioCodec;

/// Content-frame-zero offset on one decoder generation's PCM clock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Origin {
    frames: i64,
}

impl Origin {
    /// Derive the generation origin from one decoded chunk.
    pub(crate) fn of(chunk: &PcmChunk, codec: Option<AudioCodec>) -> Self {
        let clock = frame_at(chunk.meta.timestamp, chunk.meta.spec.sample_rate.get());
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

/// Convert a nanosecond timestamp to the nearest frame.
fn frame_at(at: Duration, sample_rate: u32) -> i64 {
    let frames = at
        .as_nanos()
        .saturating_mul(u128::from(sample_rate))
        .saturating_add(500_000_000)
        / 1_000_000_000;
    i64::try_from(frames).unwrap_or(i64::MAX)
}

/// How far a codec's container runs ahead of the music it carries.
fn content_origin(codec: Option<AudioCodec>) -> i64 {
    codec.map_or(0, |codec| {
        i64::try_from(AudioCodec::encoder_priming_frames(codec)).unwrap_or(0)
    })
}

/// Convert a track-timeline position to the target codec's container clock.
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
