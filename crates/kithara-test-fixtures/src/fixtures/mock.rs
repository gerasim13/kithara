use kithara_test_macros as kithara;

use crate::{assets, fixtures::samples};

/// Prepared ring handoff sample values.
#[kithara::fixture]
#[must_use]
pub fn ring_pcm() -> Vec<f32> {
    samples(&assets::mock_pcm_ring())
}

/// Prepared planar channels with poisoned padding.
#[kithara::fixture]
#[must_use]
pub fn planar_pcm() -> Vec<f32> {
    samples(&assets::mock_pcm_planar())
}

/// Encoded packet token for decoder test doubles.
#[kithara::fixture]
#[must_use]
pub fn zero_packet() -> &'static [u8] {
    assets::mock_packet_zero().bytes()
}

/// Encoded packet token for decoder test doubles.
#[kithara::fixture]
#[must_use]
pub fn one_packet() -> &'static [u8] {
    assets::mock_packet_one().bytes()
}

/// Four distinguishable MPEG frames for interrupted reads.
#[kithara::fixture]
#[must_use]
pub fn mpeg_four() -> &'static [u8] {
    assets::mpeg_frames_four().bytes()
}

/// Eight distinguishable MPEG frames for interrupted seeks.
#[kithara::fixture]
#[must_use]
pub fn mpeg_eight() -> &'static [u8] {
    assets::mpeg_frames_eight().bytes()
}

/// Prepared ascending i16 PCM samples for partial encoder input tests.
#[kithara::fixture]
#[must_use]
pub fn i16_ramp() -> Vec<i16> {
    assets::mock_packet_ramp()
        .bytes()
        .chunks_exact(2)
        .map(|sample| i16::from_le_bytes([sample[0], sample[1]]))
        .collect()
}
