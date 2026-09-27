use std::{num::NonZeroUsize, ops::Range};

use crate::backend::{gather, platform, scatter};

/// Interleaves `input_range` of every `input` channel into `output`, which
/// holds `num_out_channels` samples per frame.
///
/// Matches `fast_interleave::interleave_variable` for `f32`: slots of output
/// channels past `input.len()` stay untouched and input channels past
/// `num_out_channels` are ignored. Where `fast_interleave` panics on a short
/// slice, this interleaves the whole frames every slice holds and leaves the
/// rest of `output` untouched.
pub fn interleave_variable<Vin: AsRef<[f32]>>(
    input: &[Vin],
    input_range: Range<usize>,
    output: &mut [f32],
    num_out_channels: NonZeroUsize,
) {
    let channels = input.get(..num_out_channels.get()).unwrap_or(input);
    let frames = channels.iter().fold(
        input_range
            .end
            .saturating_sub(input_range.start)
            .min(output.len() / num_out_channels),
        |frames, channel| frames.min(channel.as_ref().len().saturating_sub(input_range.start)),
    );
    let Some(output) = frames
        .checked_mul(num_out_channels.get())
        .and_then(|samples| output.get_mut(..samples))
    else {
        return;
    };
    let window = input_range.start..input_range.start.saturating_add(frames);
    match (num_out_channels.get(), channels) {
        (1, [mono]) => {
            if let Some(mono) = mono.as_ref().get(window) {
                output.copy_from_slice(mono);
            }
        }
        (2, [left, right]) => {
            if let (Some(left), Some(right)) = (
                left.as_ref().get(window.clone()),
                right.as_ref().get(window),
            ) {
                let _ = platform::interleave_pair(left, right, output);
            }
        }
        (_, channels) => {
            for (channel, plane) in channels.iter().enumerate() {
                if let (Some(plane), Some(slots)) = (
                    plane.as_ref().get(window.clone()),
                    output.get_mut(channel..),
                ) {
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
        output_range
            .end
            .saturating_sub(output_range.start)
            .min(input.len() / num_in_channels),
        |frames, channel| frames.min(channel.as_mut().len().saturating_sub(output_range.start)),
    );
    let Some(input) = frames
        .checked_mul(num_in_channels.get())
        .and_then(|samples| input.get(..samples))
    else {
        return;
    };
    let window = output_range.start..output_range.start.saturating_add(frames);
    match (num_in_channels.get(), channels) {
        (1, [mono]) => {
            if let Some(mono) = mono.as_mut().get_mut(window) {
                mono.copy_from_slice(input);
            }
        }
        (2, [left, right]) => {
            if let (Some(left), Some(right)) = (
                left.as_mut().get_mut(window.clone()),
                right.as_mut().get_mut(window),
            ) {
                let _ = platform::deinterleave_pair(input, left, right);
            }
        }
        (_, channels) => {
            for (channel, plane) in channels.iter_mut().enumerate() {
                if let (Some(plane), Some(samples)) =
                    (plane.as_mut().get_mut(window.clone()), input.get(channel..))
                {
                    let _ = gather(samples, num_in_channels, plane);
                }
            }
        }
    }
}
