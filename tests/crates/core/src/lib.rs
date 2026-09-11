#![forbid(unsafe_code)]

//! Deterministic PCM inputs shared by low-level unit tests.

#[must_use]
pub fn trim_ramp() -> Vec<f32> {
    (0_u16..4096).map(f32::from).collect()
}

#[must_use]
pub fn silence_pcm() -> Vec<f32> {
    vec![0.0; 6]
}

#[must_use]
pub fn pcm_ramp() -> Vec<f32> {
    (1_u16..=129).map(f32::from).collect()
}

#[must_use]
pub fn negative_pcm_ramp() -> Vec<f32> {
    (1_u16..=129).map(|value| -f32::from(value)).collect()
}

#[must_use]
pub fn stereo_pair() -> Vec<f32> {
    vec![1.0, 3.0, 2.0, 6.0]
}

#[must_use]
pub fn channel_signals() -> [Vec<f32>; 9] {
    [1_u16, 2, 3, 4, 5, 6, 7, 8, 9].map(|channels| {
        (0_u16..5)
            .flat_map(|frame| (0..channels).map(move |channel| f32::from(frame * 16 + channel)))
            .collect()
    })
}
