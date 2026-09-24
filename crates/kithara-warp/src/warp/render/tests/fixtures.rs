use std::num::NonZero;

use kithara_platform::{sync::Arc, time::Duration};
use kithara_signal::{AudioChunk, AudioChunkInfo, AudioSpec};
use realfft::RealFftPlanner;

use super::super::{StretchControls, WarpRenderer as GenericWarpRenderer};
use crate::{
    Warp, WarpConfig,
    test_pools::{Pools, TestPools, pools, sample_buffer},
};

pub(super) type WarpRenderer = GenericWarpRenderer<TestPools>;

pub(super) struct Consts;

impl Consts {
    pub(super) const CH: u16 = 2;
    pub(super) const F0: f64 = 440.0;
    /// FFT length for the pitch (dominant-frequency) check.
    pub(super) const N: usize = 1 << 14;
    pub(super) const SR: u32 = 44_100;
}

pub(super) fn f64_of(x: usize) -> f64 {
    num_traits::cast(x).unwrap_or_default()
}

pub(super) fn chunk(pools: &Pools, samples: &[f32]) -> AudioChunk {
    let frames = samples.len() / usize::from(Consts::CH);
    AudioChunk::new(
        AudioChunkInfo {
            spec: AudioSpec {
                channels: Consts::CH,
                sample_rate: NonZero::new(Consts::SR).unwrap(),
            },
            frames: u32::try_from(frames).unwrap_or(0),
            timestamp: Duration::ZERO,
            ..Default::default()
        },
        sample_buffer(pools, samples),
    )
}

/// Index of the strongest spectral bin (skipping DC) of a mono window
/// taken from the middle of `mono`.
pub(super) fn dominant_bin(mono: &[f32]) -> usize {
    let start = (mono.len().saturating_sub(Consts::N)) / 2;
    let seg = &mono[start..start + Consts::N];
    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(Consts::N);
    let mut input = fft.make_input_vec();
    input.copy_from_slice(seg);
    let mut spectrum = fft.make_output_vec();
    fft.process(&mut input, &mut spectrum).unwrap();
    spectrum
        .iter()
        .enumerate()
        .skip(1)
        .max_by(|a, b| a.1.norm().total_cmp(&b.1.norm()))
        .map_or(0, |(i, _)| i)
}

pub(super) fn expected_bin(freq: f64) -> usize {
    num_traits::cast((freq * f64_of(Consts::N) / f64::from(Consts::SR)).round()).unwrap_or(0)
}

pub(super) fn spec() -> AudioSpec {
    AudioSpec {
        channels: Consts::CH,
        sample_rate: NonZero::new(Consts::SR).unwrap(),
    }
}

pub(super) fn renderer(controls: Arc<StretchControls>) -> WarpRenderer {
    let config = WarpConfig::builder().stretch(controls).build();
    Warp::new((), &config).renderer(spec(), pools())
}

pub(super) fn render_serviced(fx: &mut WarpRenderer, input: AudioChunk) -> Option<AudioChunk> {
    fx.prepare(spec());
    let output = fx
        .render(input)
        .continue_value()
        .expect("whole source span");
    fx.prepare(spec());
    output
}

pub(super) fn flush_serviced(fx: &mut WarpRenderer) -> Option<AudioChunk> {
    fx.prepare(spec());
    let output = fx.flush();
    fx.prepare(spec());
    output
}
