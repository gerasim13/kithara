const TRIM_RAMP: [f32; 4096] = ramp(0.0, false);
const PCM_RAMP: [f32; 129] = ramp(1.0, false);
const NEGATIVE_PCM_RAMP: [f32; 129] = ramp(1.0, true);
const SILENCE: [f32; 6] = [0.0; 6];
const STEREO_PAIR: [f32; 4] = [1.0, 3.0, 2.0, 6.0];
const CHANNEL_SIGNALS: [&[f32]; 9] = [
    &channel_labels::<5>(1),
    &channel_labels::<10>(2),
    &channel_labels::<15>(3),
    &channel_labels::<20>(4),
    &channel_labels::<25>(5),
    &channel_labels::<30>(6),
    &channel_labels::<35>(7),
    &channel_labels::<40>(8),
    &channel_labels::<45>(9),
];

const fn ramp<const N: usize>(first: f32, negative: bool) -> [f32; N] {
    let mut samples = [0.0; N];
    let mut index = 0;
    let mut value = first;
    while index < N {
        samples[index] = if negative { -value } else { value };
        index += 1;
        value += 1.0;
    }
    samples
}

const fn channel_labels<const N: usize>(channels: usize) -> [f32; N] {
    let mut samples = [0.0; N];
    let mut index = 0;
    let mut frame = 0;
    let mut frame_label = 0.0;
    while frame < 5 {
        let mut channel = 0;
        let mut label = frame_label;
        while channel < channels {
            samples[index] = label;
            index += 1;
            channel += 1;
            label += 1.0;
        }
        frame += 1;
        frame_label += 16.0;
    }
    samples
}

#[must_use]
pub fn trim_ramp() -> Vec<f32> {
    TRIM_RAMP.to_vec()
}

#[must_use]
pub fn silence_pcm() -> Vec<f32> {
    SILENCE.to_vec()
}

#[must_use]
pub fn pcm_ramp() -> Vec<f32> {
    PCM_RAMP.to_vec()
}

#[must_use]
pub fn negative_pcm_ramp() -> Vec<f32> {
    NEGATIVE_PCM_RAMP.to_vec()
}

#[must_use]
pub fn stereo_pair() -> Vec<f32> {
    STEREO_PAIR.to_vec()
}

#[must_use]
pub fn channel_signals() -> [Vec<f32>; 9] {
    CHANNEL_SIGNALS.map(<[f32]>::to_vec)
}
