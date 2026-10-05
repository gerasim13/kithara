use kithara_apple::audio_toolbox::AudioStreamBasicDescription;

use super::super::consts as apple_consts;
use crate::{DecodeError, DecodeResult};

mod consts {
    pub(super) const SRC_OUTPUT_MARGIN_FRAMES: u32 = 1;
}

pub(super) const fn resolve_output_sample_rate(
    source_rate: u32,
    target_output_rate: Option<u32>,
) -> u32 {
    match target_output_rate {
        Some(rate) if rate != source_rate => rate,
        _ => source_rate,
    }
}

pub(super) fn ceil_resampled_frames(
    input_frames: u32,
    source_rate: u32,
    output_rate: u32,
) -> DecodeResult<u32> {
    if source_rate == 0 {
        return Err(DecodeError::InvalidSampleRate {
            resource: "apple.codec.source",
        });
    }
    if output_rate == 0 {
        return Err(DecodeError::InvalidSampleRate {
            resource: "apple.codec.output",
        });
    }

    let numerator = u128::from(input_frames) * u128::from(output_rate);
    let divisor = u128::from(source_rate);
    let frames = numerator.div_ceil(divisor);
    u32::try_from(frames).map_err(|_| DecodeError::InvalidData {
        detail: "apple output frame capacity overflow",
    })
}

pub(super) fn output_frame_capacity(
    input_frames: u32,
    source_rate: u32,
    output_rate: u32,
) -> DecodeResult<u32> {
    let frames = ceil_resampled_frames(input_frames, source_rate, output_rate)?;
    let margin = if source_rate == output_rate {
        0
    } else {
        consts::SRC_OUTPUT_MARGIN_FRAMES
    };
    frames.checked_add(margin).ok_or(DecodeError::InvalidData {
        detail: "apple output frame capacity overflow",
    })
}

pub(super) fn output_sample_capacity(frames: u32, channels: usize) -> DecodeResult<usize> {
    usize::try_from(frames)?
        .checked_mul(channels)
        .ok_or(DecodeError::InvalidData {
            detail: "apple output sample capacity overflow",
        })
}

pub(super) fn build_pcm_output_format(
    source_rate: u32,
    channels: u16,
    target_output_rate: Option<u32>,
) -> AudioStreamBasicDescription {
    let sample_rate = resolve_output_sample_rate(source_rate, target_output_rate);
    AudioStreamBasicDescription {
        sample_rate: f64::from(sample_rate),
        format_id: apple_consts::FORMAT_LINEAR_PCM,
        format_flags: apple_consts::FORMAT_FLAGS_NATIVE_FLOAT_PACKED,
        bytes_per_packet: apple_consts::BYTES_PER_F32_SAMPLE * u32::from(channels),
        frames_per_packet: 1,
        bytes_per_frame: apple_consts::BYTES_PER_F32_SAMPLE * u32::from(channels),
        channels_per_frame: u32::from(channels),
        bits_per_channel: apple_consts::BITS_PER_F32_SAMPLE,
        ..Default::default()
    }
}
