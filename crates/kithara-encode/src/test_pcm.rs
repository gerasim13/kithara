use std::mem::size_of;

use crate::PcmSource;

/// One saw period. [`TestPcm::sawtooth`] advances by one `i16` unit per frame,
/// so a period is the number of distinct `i16` values.
///
/// `kithara-test-fixtures` states the same fact as `signal::SAW_PERIOD`, but it
/// reaches this crate through a build-dependency. That edge cannot run the other
/// way, so both sides derive the number from the type instead of sharing a
/// constant.
const SAWTOOTH_PERIOD: u32 = 1 << u16::BITS;
/// Offset from the unsigned phase onto the signed sample it renders.
const SAWTOOTH_CENTER: i32 = 1 << (i16::BITS - 1);
/// Full-scale `i16` magnitude: the divisor that maps a sample onto `[-1, 1]`.
const I16_SCALE: f32 = 32_768.0;

pub(crate) struct TestPcm {
    bytes: Vec<u8>,
    channels: u16,
    sample_rate: u32,
}

impl TestPcm {
    #[cfg(feature = "ffmpeg")]
    pub(crate) fn from_bytes(bytes: Vec<u8>, sample_rate: u32, channels: u16) -> Self {
        Self {
            bytes,
            channels,
            sample_rate,
        }
    }

    #[cfg(feature = "ffmpeg")]
    pub(crate) fn from_samples(samples: &[i16], sample_rate: u32, channels: u16) -> Self {
        Self::from_bytes(
            samples
                .iter()
                .flat_map(|sample| sample.to_le_bytes())
                .collect(),
            sample_rate,
            channels,
        )
    }

    pub(crate) fn samples_f32(&self) -> Vec<f32> {
        self.bytes
            .chunks_exact(size_of::<i16>())
            .map(|pair| f32::from(i16::from_le_bytes([pair[0], pair[1]])) / I16_SCALE)
            .collect()
    }

    pub(crate) fn sawtooth(frames: usize, sample_rate: u32, channels: u16) -> Self {
        let mut bytes = Vec::with_capacity(frames * usize::from(channels) * size_of::<i16>());
        for frame in 0..frames {
            let phase = u32::try_from(frame).expect("frame index fits u32") % SAWTOOTH_PERIOD;
            let centered = i32::try_from(phase).expect("sawtooth phase fits i32") - SAWTOOTH_CENTER;
            let sample = i16::try_from(centered).expect("sawtooth sample fits i16");
            for _ in 0..channels {
                bytes.extend_from_slice(&sample.to_le_bytes());
            }
        }
        Self {
            bytes,
            channels,
            sample_rate,
        }
    }
}

impl PcmSource for TestPcm {
    fn channels(&self) -> u16 {
        self.channels
    }

    fn read_pcm_at(&self, offset: usize, buf: &mut [u8]) -> usize {
        let Some(remaining) = self.bytes.get(offset..) else {
            return 0;
        };
        let read = remaining.len().min(buf.len());
        buf[..read].copy_from_slice(&remaining[..read]);
        read
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn total_byte_len(&self) -> Option<usize> {
        Some(self.bytes.len())
    }
}
