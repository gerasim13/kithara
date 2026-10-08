use kithara_platform::time::Duration;
use kithara_signal::{AudioChunk, AudioSpec};
use num_traits::cast::ToPrimitive;
use tracing::debug;

use crate::{consts, pipeline::seek::ResumeState};

pub(crate) fn duration(spec: AudioSpec, frames: usize) -> Duration {
    let nanos = (frames as u128)
        .saturating_mul(consts::NANOS_PER_SEC)
        .saturating_div(u128::from(spec.sample_rate.get()));
    let nanos = ToPrimitive::to_u64(&nanos).unwrap_or(u64::MAX);
    Duration::from_nanos(nanos)
}

pub(crate) fn frames(spec: AudioSpec, duration: Duration) -> usize {
    let frames = duration
        .as_nanos()
        .saturating_mul(u128::from(spec.sample_rate.get()))
        .saturating_div(consts::NANOS_PER_SEC);
    assert!(
        frames <= usize::MAX as u128,
        "post-seek frame count {frames} exceeds usize::MAX for {duration:?} at {} Hz",
        spec.sample_rate
    );
    frames as usize
}

pub(crate) fn apply(mut chunk: AudioChunk, resume: Option<&mut ResumeState>) -> Option<AudioChunk> {
    let Some(resume) = resume else {
        return Some(chunk);
    };
    if !resume.trim_head {
        return Some(chunk);
    }
    let spec = chunk.spec();
    let chunk_frames = chunk.frames();
    if chunk_frames == 0 {
        return None;
    }
    let drop_frames = frames(spec, resume.target.saturating_sub(chunk.meta.timestamp));
    if drop_frames >= chunk_frames {
        return None;
    }
    debug!(
        target = ?resume.target,
        chunk_at = ?chunk.meta.timestamp,
        frame_offset = chunk.meta.frame_offset,
        drop_frames,
        "trimmed the head of a resumed generation"
    );
    trim_start(&mut chunk, drop_frames);
    resume.trim_head = false;
    Some(chunk)
}

pub(crate) fn apply_frames(mut chunk: AudioChunk, remaining: &mut u64) -> Option<AudioChunk> {
    if *remaining == 0 {
        return Some(chunk);
    }
    let chunk_frames = u64::try_from(chunk.frames()).unwrap_or(u64::MAX);
    if chunk_frames <= *remaining {
        *remaining = remaining.saturating_sub(chunk_frames);
        return None;
    }
    let drop_frames = usize::try_from(*remaining).unwrap_or(usize::MAX);
    trim_start(&mut chunk, drop_frames);
    *remaining = 0;
    Some(chunk)
}

fn trim_start(chunk: &mut AudioChunk, drop_frames: usize) {
    let spec = chunk.spec();
    let channels = usize::from(spec.channels.max(1));
    let drop_samples = drop_frames.saturating_mul(channels);
    let len = chunk.samples.len();
    chunk.samples.copy_within(drop_samples..len, 0);
    chunk.samples.truncate(len - drop_samples);
    chunk.meta.frame_offset = chunk.meta.frame_offset.saturating_add(drop_frames as u64);
    chunk.meta.timestamp = chunk
        .meta
        .timestamp
        .saturating_add(duration(spec, drop_frames));
    chunk.meta.frames = chunk
        .meta
        .frames
        .saturating_sub(u32::try_from(drop_frames).unwrap_or(u32::MAX));
}
