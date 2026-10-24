use kithara_test_macros as kithara;

use crate::{assets, fixtures::samples};

#[kithara::fixture]
#[must_use]
pub fn shifted_pitch() -> Vec<f32> {
    samples(&assets::shifted_pitch_default())
}

#[kithara::fixture]
#[must_use]
pub fn quality_control_a() -> Vec<f32> {
    samples(&assets::quality_control_a())
}

#[kithara::fixture]
#[must_use]
pub fn quality_control_b() -> Vec<f32> {
    samples(&assets::quality_control_b())
}

#[kithara::fixture]
#[must_use]
pub fn quality_control_joined() -> Vec<f32> {
    samples(&assets::quality_control_joined())
}

#[kithara::fixture]
#[must_use]
pub fn oracle_stem_a() -> Vec<f32> {
    samples(&assets::oracle_stem_a_default())
}

#[kithara::fixture]
#[must_use]
pub fn oracle_stem_b() -> Vec<f32> {
    samples(&assets::oracle_stem_b_default())
}

#[kithara::fixture]
#[must_use]
pub fn phase_sine_measured() -> Vec<f32> {
    samples(&assets::phase_sine_measured())
}

#[kithara::fixture]
#[must_use]
pub fn phase_sine_jitter() -> Vec<f32> {
    samples(&assets::phase_sine_jitter())
}

#[kithara::fixture]
#[must_use]
pub fn phase_sine_anchor() -> Vec<f32> {
    samples(&assets::phase_sine_anchor())
}

#[kithara::fixture]
#[must_use]
pub fn phase_sine_dropped() -> Vec<f32> {
    samples(&assets::phase_sine_dropped())
}

#[kithara::fixture]
#[must_use]
pub fn phase_noise() -> Vec<f64> {
    assets::phase_noise_default()
        .bytes()
        .as_chunks::<{ size_of::<f64>() }>()
        .0
        .iter()
        .map(|bytes| f64::from_le_bytes(*bytes))
        .collect()
}

#[kithara::fixture]
#[must_use]
pub fn listening_reference() -> Vec<f32> {
    samples(&assets::listening_reference_default())
}

#[kithara::fixture]
#[must_use]
pub fn cochlea_control() -> Vec<f32> {
    samples(&assets::cochlea_signal_control())
}

#[kithara::fixture]
#[must_use]
pub fn cochlea_loudness() -> Vec<f32> {
    samples(&assets::cochlea_signal_loudness())
}

/// Prepared six-beat input at one of the declared rates and tempos.
/// # Panics
/// Panics when the requested input is undeclared or unavailable.
#[must_use]
pub fn cochlea_pulses(sample_rate: u32, bpm: f64) -> Vec<f32> {
    let tempo = bpm.to_string().replace('.', "_");
    let name = format!("cochlea_pulses_rate_{sample_rate}_bpm_{tempo}");
    let asset =
        assets::by_name(&name).unwrap_or_else(|| panic!("no prepared pulse fixture `{name}`"));
    samples(&asset)
}

/// Prepared three seconds of mono silence at 48000 Hz.
#[kithara::fixture]
#[must_use]
pub fn cochlea_silence() -> Vec<f32> {
    samples(&assets::cochlea_silence_default())
}
