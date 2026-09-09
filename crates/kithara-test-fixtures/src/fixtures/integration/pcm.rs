use kithara_test_macros as kithara;

use crate::{assets, fixtures::samples};

#[kithara::fixture]
#[must_use]
pub fn broadcast_tone() -> Vec<f32> {
    samples(&assets::broadcast_tone_default())
}

#[kithara::fixture]
#[must_use]
pub fn default_pcm() -> Vec<f32> {
    samples(&assets::default_pcm_default())
}

#[kithara::fixture]
#[must_use]
pub fn allocation_ramp() -> Vec<f32> {
    samples(&assets::allocation_ramp_default())
}

#[kithara::fixture]
#[must_use]
pub fn allocation_planar() -> Vec<f32> {
    samples(&assets::allocation_planar_default())
}

#[kithara::fixture]
#[must_use]
pub fn allocation_sequence() -> Vec<f32> {
    samples(&assets::allocation_sequence_default())
}

#[kithara::fixture]
#[must_use]
pub fn stream_sine() -> Vec<f32> {
    samples(&assets::stream_sine_default())
}

#[kithara::fixture]
#[must_use]
pub fn dsp_tone_a440() -> Vec<f32> {
    samples(&assets::dsp_tone_a440())
}

#[kithara::fixture]
#[must_use]
pub fn dsp_tone_a220() -> Vec<f32> {
    samples(&assets::dsp_tone_a220())
}

#[kithara::fixture]
#[must_use]
pub fn dsp_tone_unity() -> Vec<f32> {
    samples(&assets::dsp_tone_unity())
}

#[kithara::fixture]
#[must_use]
pub fn dsp_tone_ceiling() -> Vec<f32> {
    samples(&assets::dsp_tone_ceiling())
}

#[kithara::fixture]
#[must_use]
pub fn dsp_tone_ceiling_unity() -> Vec<f32> {
    samples(&assets::dsp_tone_ceiling_unity())
}

#[kithara::fixture]
#[must_use]
pub fn dsp_tone_ceiling_low() -> Vec<f32> {
    samples(&assets::dsp_tone_ceiling_low())
}

#[kithara::fixture]
#[must_use]
pub fn dsp_tone_round_low() -> Vec<f32> {
    samples(&assets::dsp_tone_round_low())
}

#[kithara::fixture]
#[must_use]
pub fn dsp_tone_round_mid() -> Vec<f32> {
    samples(&assets::dsp_tone_round_mid())
}

#[kithara::fixture]
#[must_use]
pub fn dsp_tone_round_poison() -> Vec<f32> {
    samples(&assets::dsp_tone_round_poison())
}

#[kithara::fixture]
#[must_use]
pub fn dsp_silence() -> Vec<f32> {
    samples(&assets::dsp_silence_default())
}

#[kithara::fixture]
#[must_use]
pub fn dsp_sweep() -> Vec<f32> {
    samples(&assets::dsp_sweep_default())
}

#[kithara::fixture]
#[must_use]
pub fn gapless_sine_first() -> Vec<f32> {
    samples(&assets::gapless_sine_first())
}

#[kithara::fixture]
#[must_use]
pub fn gapless_sine_second() -> Vec<f32> {
    samples(&assets::gapless_sine_second())
}

#[kithara::fixture]
#[must_use]
pub fn gapless_sine_whole() -> Vec<f32> {
    samples(&assets::gapless_sine_whole())
}

#[kithara::fixture]
#[must_use]
pub fn origin_tone() -> Vec<f32> {
    samples(&assets::origin_tone_default())
}

#[kithara::fixture]
#[must_use]
pub fn packaging_tone() -> Vec<f32> {
    samples(&assets::packaging_tone_default())
}

#[kithara::fixture]
#[must_use]
pub fn perf_interleaved() -> Vec<f32> {
    samples(&assets::perf_resampler_interleaved())
}

#[kithara::fixture]
#[must_use]
pub fn perf_planar() -> [Vec<f32>; 2] {
    [
        samples(&assets::perf_resampler_left()),
        samples(&assets::perf_resampler_right()),
    ]
}
