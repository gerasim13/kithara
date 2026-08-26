use std::num::NonZero;

use kithara_bufpool::PcmPool;
use kithara_decode::{PcmChunk, PcmMeta, PcmSpec};
use kithara_platform::{sync::Arc, time::Duration};
use realfft::RealFftPlanner;

use super::{StretchControls, TimeStretchProcessor};
use crate::traits::AudioEffect;

mod playback;
mod target;
mod timeline;

struct Consts;

impl Consts {
    const CH: u16 = 2;
    const F0: f64 = 440.0;
    /// FFT length for the pitch (dominant-frequency) check.
    const N: usize = 1 << 14;
    const SR: u32 = 44_100;
}

fn f32_of(x: f64) -> f32 {
    num_traits::cast(x).unwrap_or_default()
}

fn f64_of(x: usize) -> f64 {
    num_traits::cast(x).unwrap_or_default()
}

/// Interleaved stereo sine at `F0`, phase-accumulated to avoid drift.
fn sine(frames: usize) -> Vec<f32> {
    let inc = std::f64::consts::TAU * Consts::F0 / f64::from(Consts::SR);
    let mut phase = 0.0_f64;
    let mut out = Vec::with_capacity(frames * usize::from(Consts::CH));
    for _ in 0..frames {
        let s = f32_of(0.5 * phase.sin());
        out.push(s);
        out.push(s);
        phase += inc;
    }
    out
}

fn chunk(samples: &[f32]) -> PcmChunk {
    let frames = samples.len() / usize::from(Consts::CH);
    PcmChunk::new(
        PcmMeta {
            spec: PcmSpec {
                channels: Consts::CH,
                sample_rate: NonZero::new(Consts::SR).unwrap(),
            },
            frames: u32::try_from(frames).unwrap_or(0),
            timestamp: Duration::ZERO,
            ..Default::default()
        },
        PcmPool::default().attach(samples.to_vec()),
    )
}

/// Index of the strongest spectral bin (skipping DC) of a mono window
/// taken from the middle of `mono`.
fn dominant_bin(mono: &[f32]) -> usize {
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

fn expected_bin(freq: f64) -> usize {
    num_traits::cast((freq * f64_of(Consts::N) / f64::from(Consts::SR)).round()).unwrap_or(0)
}

fn spec() -> PcmSpec {
    PcmSpec {
        channels: Consts::CH,
        sample_rate: NonZero::new(Consts::SR).unwrap(),
    }
}

fn processor(controls: Arc<StretchControls>) -> TimeStretchProcessor {
    TimeStretchProcessor::new(controls, spec(), PcmPool::default())
}

fn process_serviced(fx: &mut TimeStretchProcessor, input: PcmChunk) -> Option<PcmChunk> {
    fx.service_deferred(spec());
    let output = fx.process(input);
    fx.service_deferred(spec());
    output
}

fn flush_serviced(fx: &mut TimeStretchProcessor) -> Option<PcmChunk> {
    fx.service_deferred(spec());
    let output = fx.flush();
    fx.service_deferred(spec());
    output
}
