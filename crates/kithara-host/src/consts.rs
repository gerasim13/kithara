use std::num::NonZeroU32;

pub(crate) const DEFAULT_SAMPLE_RATE: NonZeroU32 = match NonZeroU32::new(44_100) {
    Some(sample_rate) => sample_rate,
    None => unreachable!(),
};

#[cfg(test)]
pub(crate) const GRAPH_BLOCK_FRAMES: usize = 128;

#[cfg(test)]
pub(crate) const STEREO_CHANNELS: usize = 2;

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const RING_ADMISSION_SAMPLE_RATE: u32 = 48_000;

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const RING_ADMISSION_BLOCK_FRAMES: u32 = 512;

#[cfg(test)]
pub(crate) const TRANSPORT_BLOCK_FRAMES: usize = 480;

#[cfg(test)]
pub(crate) const TRANSPORT_SAMPLE_RATE: u32 = 48_000;

/// Peak of the Host metronome's downbeat click.
pub(crate) const DEFAULT_METRONOME_LEVEL: f32 = 0.5;

/// Depth of the Host metronome's duck: the mix is muted at the peak of every
/// click.
pub(crate) const DEFAULT_METRONOME_DUCK: f32 = 1.0;
