/// Measurements collected from one offline render window.
#[non_exhaustive]
pub struct WindowStats {
    /// Silent blocks in the window.
    pub silent_blocks: u32,
    /// Total blocks in the window.
    pub total_blocks: u32,
    /// First sample of the window in the output buffer.
    pub window_start_sample: usize,
}

impl WindowStats {
    #[must_use]
    pub const fn new(silent_blocks: u32, total_blocks: u32, window_start_sample: usize) -> Self {
        Self {
            silent_blocks,
            total_blocks,
            window_start_sample,
        }
    }
}

/// Copy the left channel from interleaved PCM samples.
#[must_use]
pub fn deinterleave_left(samples: &[f32], channels: usize) -> Vec<f32> {
    samples
        .chunks_exact(channels)
        .map(|frame| frame[0])
        .collect()
}

/// Root mean square of an interleaved sample slice.
#[must_use]
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "sample count precision is adequate for test windows"
    )]
    let count = samples.len() as f32;
    let sum_sq: f32 = samples.iter().map(|sample| sample * sample).sum();
    (sum_sq / count).sqrt()
}
