use std::num::NonZeroU128;

use kithara_decode::{DecodeError, DecodeResult};
use kithara_platform::time::Duration;
use kithara_signal::{AudioChunk, AudioSpec, SourceSpan};
use tracing::debug;

use crate::pipeline::seek::{ResumeState, ResumeTarget};

fn frames(spec: AudioSpec, duration: Duration) -> DecodeResult<usize> {
    let frames = spec.frame_at(duration).map_err(|_| mapping_error())?;
    usize::try_from(frames).map_err(|_| mapping_error())
}

pub(crate) fn apply(
    mut chunk: AudioChunk,
    resume: Option<&mut ResumeState>,
) -> DecodeResult<Option<AudioChunk>> {
    let Some(resume) = resume else {
        return Ok(Some(chunk));
    };
    if !resume.trim_head {
        return Ok(Some(chunk));
    }
    let spec = chunk.spec();
    let chunk_frames = chunk.frames();
    if chunk_frames == 0 {
        return Ok(None);
    }
    let drop_frames = match resume.target {
        ResumeTarget::Position(target) => {
            frames(spec, target.saturating_sub(chunk.meta.timestamp))?
        }
        ResumeTarget::Source(end) => {
            let target = output_frame(end, spec)?;
            usize::try_from(target.saturating_sub(chunk.meta.frame_offset))
                .map_err(|_| mapping_error())?
        }
    };
    if drop_frames >= chunk_frames {
        return Ok(None);
    }
    debug!(
        target = ?resume.target,
        chunk_at = ?chunk.meta.timestamp,
        frame_offset = chunk.meta.frame_offset,
        drop_frames,
        "trimmed the head of a resumed generation"
    );
    trim_start(&mut chunk, drop_frames)?;
    resume.trim_head = false;
    Ok(Some(chunk))
}

pub(crate) fn rebase_source(
    chunk: &mut AudioChunk,
    resume: Option<&ResumeState>,
) -> DecodeResult<()> {
    if let Some(ResumeState {
        target: ResumeTarget::Source(end),
        ..
    }) = resume
    {
        let spec = chunk.spec();
        let output_rate = u128::from(spec.sample_rate.get());
        let source_rate = u128::from(end.sample_rate().get());
        let offset = chunk
            .meta
            .frame_offset
            .checked_sub(output_frame(*end, spec)?)
            .ok_or_else(mapping_error)?;
        let start = u128::from(end.frame()) * output_rate + u128::from(offset) * source_rate;
        let span = SourceSpan::try_from((
            start,
            source_rate,
            NonZeroU128::from(spec.sample_rate),
            end.sample_rate(),
            u64::from(chunk.meta.frames),
        ))
        .ok()
        .ok_or_else(mapping_error)?
        .with_mapping_revision(end.mapping_revision())
        .with_render_revision(chunk.meta.render_revision);
        chunk.meta.timestamp = span.position_at(0).ok_or_else(mapping_error)?;
        chunk.meta.end_timestamp = span
            .position_at(span.output_frames())
            .ok_or_else(mapping_error)?;
        chunk.meta.mapping_revision = span.mapping_revision();
        chunk.meta.source_span = Some(span);
    }
    Ok(())
}

fn output_frame(end: crate::SourceEnd, spec: AudioSpec) -> DecodeResult<u64> {
    u64::try_from(
        u128::from(end.frame()) * u128::from(spec.sample_rate.get())
            / u128::from(end.sample_rate().get()),
    )
    .map_err(|_| mapping_error())
}

fn mapping_error() -> DecodeError {
    DecodeError::InvalidData {
        detail: "resumed source mapping does not cover PCM",
    }
}

pub(crate) fn apply_frames(
    mut chunk: AudioChunk,
    remaining: &mut u64,
) -> DecodeResult<Option<AudioChunk>> {
    if *remaining == 0 {
        return Ok(Some(chunk));
    }
    let chunk_frames = u64::try_from(chunk.frames()).unwrap_or(u64::MAX);
    if chunk_frames <= *remaining {
        *remaining = remaining.saturating_sub(chunk_frames);
        return Ok(None);
    }
    let drop_frames = usize::try_from(*remaining).unwrap_or(usize::MAX);
    trim_start(&mut chunk, drop_frames)?;
    *remaining = 0;
    Ok(Some(chunk))
}

fn trim_start(chunk: &mut AudioChunk, drop_frames: usize) -> DecodeResult<()> {
    let spec = chunk.spec();
    let frame_offset = chunk
        .meta
        .frame_offset
        .checked_add(drop_frames as u64)
        .ok_or_else(mapping_error)?;
    let source_span = chunk
        .meta
        .source_span
        .map(|span| {
            span.for_output_range(drop_frames as u64..u64::from(chunk.meta.frames))
                .ok_or_else(mapping_error)
        })
        .transpose()?;
    let timestamp = if let Some(span) = source_span {
        span.position_at(0).ok_or_else(mapping_error)?
    } else {
        let origin = chunk
            .meta
            .timestamp
            .saturating_sub(spec.duration_for(chunk.meta.frame_offset)?);
        origin
            .checked_add(spec.duration_for(frame_offset)?)
            .ok_or_else(mapping_error)?
    };
    let channels = usize::from(spec.channels.max(1));
    let drop_samples = drop_frames.saturating_mul(channels);
    let len = chunk.samples.len();
    chunk.samples.copy_within(drop_samples..len, 0);
    chunk.samples.truncate(len - drop_samples);
    chunk.meta.frame_offset = frame_offset;
    chunk.meta.timestamp = timestamp;
    chunk.meta.source_span = source_span;
    chunk.meta.frames = chunk
        .meta
        .frames
        .saturating_sub(u32::try_from(drop_frames).unwrap_or(u32::MAX));
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use kithara_test_utils::kithara;

    use super::*;
    use crate::test_pools::{pools, sample_buffer};

    #[kithara::test]
    fn a_seek_to_a_nanosecond_rounded_frame_retains_that_pcm_frame() {
        let pools = pools();
        let rate = NonZeroU32::new(44_100).expect("fixture rate");
        let span = SourceSpan::new(0, 4, rate, 4).expect("fixture source span");
        let pcm = [0.25, -0.25, 0.5, -0.5, 0.75, -0.75, 1.0, -1.0];
        let chunk = AudioChunk::new(
            kithara_signal::AudioChunkInfo {
                spec: AudioSpec::new(2, rate),
                frames: 4,
                end_timestamp: Duration::from_nanos(90_702),
                source_span: Some(span),
                ..kithara_signal::AudioChunkInfo::default()
            },
            sample_buffer(&pools, &pcm),
        );
        let mut resume = ResumeState {
            target: ResumeTarget::Position(Duration::from_nanos(22_675)),
            trim_head: true,
        };

        let retained = apply(chunk, Some(&mut resume))
            .expect("seek retains a source interval")
            .expect("three PCM frames remain");

        assert_eq!(retained.samples.as_ref(), &pcm[2..]);
        assert_eq!(retained.meta.frame_offset, 1);
        assert_eq!(retained.meta.timestamp, Duration::from_nanos(22_675));
        assert_eq!(retained.meta.frames, 3);
        assert_eq!(retained.meta.source_span, span.for_output_range(1..4));
        assert!(!resume.trim_head);
    }

    #[kithara::test]
    #[case::native_before(false, 38_249_999, 0)]
    #[case::native_tie(false, 38_250_000, 1)]
    #[case::native_after(false, 38_250_001, 1)]
    #[case::mapped_before(true, 38_249_999, 0)]
    #[case::mapped_tie(true, 38_250_000, 1)]
    #[case::mapped_after(true, 38_250_001, 1)]
    fn a_seek_rounds_relative_to_its_native_pcm_basis(
        #[case] mapped: bool,
        #[case] target_ns: u64,
        #[case] drop_frames: usize,
    ) {
        let pools = pools();
        let rate = NonZeroU32::new(1_000).expect("fixture rate");
        let span = SourceSpan::try_from((
            151,
            4,
            NonZeroU128::new(4).expect("fixture denominator"),
            rate,
            4,
        ))
        .expect("fractional native PCM basis");
        let pcm = [0.25, -0.25, 0.5, -0.5, 0.75, -0.75, 1.0, -1.0];
        let chunk = AudioChunk::new(
            kithara_signal::AudioChunkInfo {
                spec: AudioSpec::new(2, rate),
                frame_offset: 37,
                timestamp: Duration::from_nanos(37_750_000),
                end_timestamp: Duration::from_nanos(41_750_000),
                frames: 4,
                source_span: mapped.then_some(span),
                ..kithara_signal::AudioChunkInfo::default()
            },
            sample_buffer(&pools, &pcm),
        );
        let mut resume = ResumeState {
            target: ResumeTarget::Position(Duration::from_nanos(target_ns)),
            trim_head: true,
        };

        let retained = apply(chunk, Some(&mut resume))
            .expect("native-basis seek")
            .expect("the native PCM suffix remains");
        let drop_frames_u64 = u64::try_from(drop_frames).expect("fixture frame count");

        assert_eq!(retained.samples.as_ref(), &pcm[drop_frames * 2..]);
        assert_eq!(retained.meta.frame_offset, 37 + drop_frames_u64);
        assert_eq!(
            retained.meta.timestamp,
            Duration::from_nanos(37_750_000 + drop_frames_u64 * 1_000_000)
        );
        assert_eq!(
            retained.meta.end_timestamp,
            Duration::from_nanos(41_750_000)
        );
        assert_eq!(retained.frames(), 4 - drop_frames);
        assert_eq!(
            retained.meta.source_span,
            mapped.then(|| span
                .for_output_range(drop_frames_u64..4)
                .expect("PCM suffix"))
        );
        assert!(!resume.trim_head);
    }
}
