use std::sync::OnceLock;

use kithara_test_macros as kithara;

use crate::{assets, fixtures::samples};

/// Build-time PCM banks for elastic-engine conformance tests.
#[non_exhaustive]
pub struct StretchPcm {
    /// Prepared square samples.
    pub square: Vec<f32>,
    /// Prepared impulses samples.
    pub impulses: Vec<f32>,
    /// Prepared continuous samples.
    pub continuous: Vec<f32>,
    /// Prepared short samples.
    pub short: Vec<f32>,
    /// Prepared indexed samples.
    pub indexed: Vec<f32>,
    /// Prepared ramp samples.
    pub ramp: Vec<f32>,
    /// Prepared bungee samples.
    pub bungee: Vec<f32>,
    /// Prepared mono samples.
    pub mono: Vec<f32>,
    /// Prepared silence samples.
    pub silence: Vec<f32>,
    /// Prepared quarter samples.
    pub quarter: Vec<f32>,
    /// Prepared nine samples.
    pub nine: Vec<f32>,
    /// Prepared fifth samples.
    pub fifth: Vec<f32>,
    /// Prepared half samples.
    pub half: Vec<f32>,
    /// Prepared four fifths samples.
    pub four_fifths: Vec<f32>,
    /// Phase-aligned landmark and terminal marker tones.
    pub tones: Vec<Vec<f32>>,
}

/// Shared prepared PCM inputs; tests select spans using measured engine latency.
#[kithara::fixture]
pub fn stretch_pcm() -> &'static StretchPcm {
    static PCM: OnceLock<StretchPcm> = OnceLock::new();
    PCM.get_or_init(|| StretchPcm {
        square: samples(&assets::stretch_pcm_square()),
        impulses: samples(&assets::stretch_pcm_impulses()),
        continuous: samples(&assets::stretch_pcm_continuous()),
        short: samples(&assets::stretch_pcm_short()),
        indexed: samples(&assets::stretch_pcm_indexed()),
        ramp: samples(&assets::stretch_pcm_ramp()),
        bungee: samples(&assets::stretch_pcm_bungee()),
        mono: samples(&assets::stretch_pcm_mono()),
        silence: samples(&assets::stretch_pcm_silence()),
        quarter: samples(&assets::stretch_pcm_quarter()),
        nine: samples(&assets::stretch_pcm_nine()),
        fifth: samples(&assets::stretch_pcm_fifth()),
        half: samples(&assets::stretch_pcm_half()),
        four_fifths: samples(&assets::stretch_pcm_four_fifths()),
        tones: samples(&assets::stretch_pcm_tones())
            .chunks_exact(65_536 * 2)
            .map(<[f32]>::to_vec)
            .collect(),
    })
}
