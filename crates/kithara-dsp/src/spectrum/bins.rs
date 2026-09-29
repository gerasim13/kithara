use crate::backend;

/// `output[k] = |re[k] + i·im[k]|` over the common prefix of the three
/// slices; returns its length. Within two epsilons of `hypot` while the
/// squares stay finite, that is below `1.8e19`.
pub fn magnitude(re: &[f32], im: &[f32], output: &mut [f32]) -> usize {
    backend::magnitude(re, im, output)
}

/// `output[k] = arg(re[k] + i·im[k])` in `[−π, π]` over the common prefix
/// of the three slices; returns its length. Within `1e-6` rad of `atan2`.
pub fn phase(re: &[f32], im: &[f32], output: &mut [f32]) -> usize {
    backend::phase(re, im, output)
}
