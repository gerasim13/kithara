use std::{num::NonZeroUsize, ops::Range};

use crate::backend::{gather, platform, scatter};

/// Interleaves `input_range` of every channel of channel-major `input`, whose
/// channel `c` fills `input[c * stride..(c + 1) * stride]`, into `output`,
/// which holds `num_out_channels` samples per frame.
///
/// Matches `fast_interleave::interleave_variable` for `f32` over
/// `input.chunks_exact(stride)`, without a slice of channel references:
/// samples past the last whole plane are ignored, slots of output channels
/// past the plane count stay untouched, and any channel count runs without
/// allocating. Where `fast_interleave` panics on a short slice, this
/// interleaves the whole frames every slice holds and leaves the rest of
/// `output` untouched.
pub fn interleave_channel_major(
    input: &[f32],
    stride: NonZeroUsize,
    input_range: Range<usize>,
    output: &mut [f32],
    num_out_channels: NonZeroUsize,
) {
    let channels = (input.len() / stride).min(num_out_channels.get());
    let mut frames = frame_bound(&input_range, output.len() / num_out_channels);
    if channels > 0 {
        frames = frames.min(stride.get().saturating_sub(input_range.start));
    }
    let Some(output) = frames
        .checked_mul(num_out_channels.get())
        .and_then(|samples| output.get_mut(..samples))
    else {
        return;
    };
    let window = input_range.start..input_range.start.saturating_add(frames);
    let mut planes = input.chunks_exact(stride.get()).take(channels);
    match (num_out_channels.get(), channels) {
        (1, 1) => {
            if let Some(mono) = planes.next().and_then(|mono| mono.get(window)) {
                output.copy_from_slice(mono);
            }
        }
        (2, 2) => {
            if let (Some(left), Some(right)) = (
                planes.next().and_then(|left| left.get(window.clone())),
                planes.next().and_then(|right| right.get(window)),
            ) {
                let _ = platform::interleave_pair(left, right, output);
            }
        }
        _ => {
            for (channel, plane) in planes.enumerate() {
                if let (Some(plane), Some(slots)) =
                    (plane.get(window.clone()), output.get_mut(channel..))
                {
                    let _ = scatter(plane, slots, num_out_channels);
                }
            }
        }
    }
}

/// Splits `input`, which holds `num_in_channels` samples per frame, into
/// `output_range` of every `output` channel.
///
/// Matches `fast_interleave::deinterleave_variable` for `f32`: output channels
/// past `num_in_channels` stay untouched and input channels past
/// `output.len()` are skipped. Where `fast_interleave` panics on a short
/// slice, this splits the whole frames every slice holds and leaves the rest
/// of `output` untouched.
pub fn deinterleave_variable<Vout: AsMut<[f32]>>(
    input: &[f32],
    num_in_channels: NonZeroUsize,
    output: &mut [Vout],
    output_range: Range<usize>,
) {
    let used = output.len().min(num_in_channels.get());
    let Some(channels) = output.get_mut(..used) else {
        return;
    };
    let frames = channels.iter_mut().fold(
        frame_bound(&output_range, input.len() / num_in_channels),
        |frames, channel| frames.min(channel.as_mut().len().saturating_sub(output_range.start)),
    );
    deinterleave_planes(
        input,
        num_in_channels,
        channels.iter_mut().map(AsMut::as_mut),
        used,
        output_range.start,
        frames,
    );
}

/// Splits `input`, which holds `num_in_channels` samples per frame, into
/// `output_range` of every channel of channel-major `output`, whose channel
/// `c` fills `output[c * stride..(c + 1) * stride]`.
///
/// Equals [`deinterleave_variable`] over `output.chunks_exact_mut(stride)`,
/// without a slice of channel references: samples past the last whole plane
/// stay untouched, and any channel count runs without allocating.
pub fn deinterleave_channel_major(
    input: &[f32],
    num_in_channels: NonZeroUsize,
    output: &mut [f32],
    stride: NonZeroUsize,
    output_range: Range<usize>,
) {
    let channels = (output.len() / stride).min(num_in_channels.get());
    let mut frames = frame_bound(&output_range, input.len() / num_in_channels);
    if channels > 0 {
        frames = frames.min(stride.get().saturating_sub(output_range.start));
    }
    deinterleave_planes(
        input,
        num_in_channels,
        output.chunks_exact_mut(stride.get()).take(channels),
        channels,
        output_range.start,
        frames,
    );
}

/// Frames of `range` that `available` whole interleaved frames can hold.
fn frame_bound(range: &Range<usize>, available: usize) -> usize {
    range.end.saturating_sub(range.start).min(available)
}

/// Splits `frames` whole frames of `input` into `channels` planes from
/// `start`; every plane holds them, and `input` holds them or the call
/// writes nothing.
fn deinterleave_planes<'a>(
    input: &[f32],
    num_in_channels: NonZeroUsize,
    mut planes: impl Iterator<Item = &'a mut [f32]>,
    channels: usize,
    start: usize,
    frames: usize,
) {
    let Some(input) = frames
        .checked_mul(num_in_channels.get())
        .and_then(|samples| input.get(..samples))
    else {
        return;
    };
    let window = start..start.saturating_add(frames);
    match (num_in_channels.get(), channels) {
        (1, 1) => {
            if let Some(mono) = planes.next().and_then(|mono| mono.get_mut(window)) {
                mono.copy_from_slice(input);
            }
        }
        (2, 2) => {
            if let (Some(left), Some(right)) = (
                planes.next().and_then(|left| left.get_mut(window.clone())),
                planes.next().and_then(|right| right.get_mut(window)),
            ) {
                let _ = platform::deinterleave_pair(input, left, right);
            }
        }
        _ => {
            for (channel, plane) in planes.enumerate() {
                if let (Some(plane), Some(samples)) =
                    (plane.get_mut(window.clone()), input.get(channel..))
                {
                    let _ = gather(samples, num_in_channels, plane);
                }
            }
        }
    }
}
