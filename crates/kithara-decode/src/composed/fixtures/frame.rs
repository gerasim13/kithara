use kithara_bufpool::SampleBuffer;
use kithara_signal::AudioSpec;

use crate::error::{DecodeError, DecodeResult};

#[cfg(test)]
pub(in crate::composed) fn write_silent_test_frame(
    pcm: &[f32],
    spec: AudioSpec,
    frames_per_call: u32,
    out: &mut SampleBuffer,
) -> DecodeResult<u32> {
    let frames = usize::try_from(frames_per_call)?;
    let samples = frames
        .checked_mul(usize::from(spec.channels))
        .ok_or_else(|| DecodeError::InvalidData {
            detail: "test frame sample count overflow",
        })?;
    out.ensure_len(samples)?;
    out[..samples].copy_from_slice(&pcm[..samples]);
    out.truncate(samples);
    Ok(frames_per_call)
}
