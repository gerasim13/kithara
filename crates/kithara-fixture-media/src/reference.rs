//! Persisted native PCM prepared with a declared audio fixture.

use std::{io, num::NonZeroU64};

/// Prepared native audio and its optional exact presentation extent.
#[derive(Debug, PartialEq)]
pub struct AudioReference {
    /// Interleaved native PCM, including samples outside presentation trimming.
    pub samples: Vec<f32>,
    /// Native samples per second.
    pub sample_rate: u32,
    /// Interleaved channels per frame.
    pub channels: u16,
    /// Exact declared presentation frame count, when the source provides one.
    pub presentation_frames: Option<NonZeroU64>,
}

impl AudioReference {
    const HEADER_BYTES: usize = 24;
    const MAGIC: &[u8; 8] = b"KITHPCM1";

    fn validate(&self) -> io::Result<()> {
        if self.sample_rate == 0
            || self.channels == 0
            || self.samples.is_empty()
            || !self
                .samples
                .len()
                .is_multiple_of(usize::from(self.channels))
            || self.samples.iter().any(|sample| !sample.is_finite())
        {
            return Err(invalid(
                "reference requires finite complete native PCM frames",
            ));
        }
        Ok(())
    }

    /// Read a prepared reference without invoking a decoder.
    ///
    /// # Errors
    /// Returns an error for an incompatible header or invalid PCM payload.
    pub fn from_bytes(bytes: &[u8]) -> io::Result<Self> {
        let header = bytes
            .get(..Self::HEADER_BYTES)
            .filter(|header| header.starts_with(Self::MAGIC))
            .ok_or_else(|| invalid("invalid audio reference header"))?;
        let frames = u64::from_le_bytes(
            header[16..24]
                .try_into()
                .map_err(|_| invalid("invalid audio reference integer"))?,
        );
        let presentation_frames = match (header[14], header[15], frames) {
            (0, 0, 0) => None,
            (1, 0, frames) => Some(
                NonZeroU64::new(frames)
                    .ok_or_else(|| invalid("reference presentation must contain frames"))?,
            ),
            _ => return Err(invalid("invalid audio reference presentation tag")),
        };
        let (samples, remainder) = bytes[Self::HEADER_BYTES..].as_chunks::<4>();
        if !remainder.is_empty() {
            return Err(invalid("audio reference has a partial sample"));
        }
        let reference = Self {
            sample_rate: u32::from_le_bytes([header[8], header[9], header[10], header[11]]),
            channels: u16::from_le_bytes([header[12], header[13]]),
            presentation_frames,
            samples: samples
                .iter()
                .map(|sample| f32::from_le_bytes(*sample))
                .collect(),
        };
        reference.validate()?;
        Ok(reference)
    }
}

impl TryFrom<&AudioReference> for Vec<u8> {
    type Error = io::Error;

    /// Serialize a validated reference in the portable fixture format.
    ///
    /// # Errors
    /// Returns an error for an invalid audio spec or PCM payload.
    fn try_from(reference: &AudioReference) -> Result<Self, Self::Error> {
        reference.validate()?;
        let mut bytes =
            Self::with_capacity(AudioReference::HEADER_BYTES + reference.samples.len() * 4);
        bytes.extend_from_slice(AudioReference::MAGIC);
        bytes.extend_from_slice(&reference.sample_rate.to_le_bytes());
        bytes.extend_from_slice(&reference.channels.to_le_bytes());
        bytes.extend_from_slice(&u16::from(reference.presentation_frames.is_some()).to_le_bytes());
        bytes.extend_from_slice(
            &reference
                .presentation_frames
                .map_or(0, NonZeroU64::get)
                .to_le_bytes(),
        );
        for sample in &reference.samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        Ok(bytes)
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::{AudioReference, NonZeroU64};

    fn reference() -> AudioReference {
        AudioReference {
            sample_rate: 44_100,
            channels: 2,
            samples: vec![0.5, -0.5, 0.25, -0.25],
            presentation_frames: NonZeroU64::new(1),
        }
    }

    #[kithara::test]
    fn reference_retains_full_pcm_and_separate_presentation_extent() {
        for frames in [None, NonZeroU64::new(1)] {
            let mut reference = reference();
            reference.presentation_frames = frames;
            let bytes = Vec::try_from(&reference).expect("valid reference");
            assert_eq!(
                AudioReference::from_bytes(&bytes).expect("prepared record"),
                reference
            );
        }
    }

    #[kithara::test]
    fn reference_rejects_malformed_records() {
        let bytes = Vec::try_from(&reference()).expect("valid reference");
        for length in [
            0,
            AudioReference::HEADER_BYTES - 1,
            AudioReference::HEADER_BYTES,
            AudioReference::HEADER_BYTES + 1,
            AudioReference::HEADER_BYTES + 4,
        ] {
            assert!(AudioReference::from_bytes(&bytes[..length]).is_err());
        }
        for (index, value) in [(0, b'?'), (14, 2), (15, 1)] {
            let mut damaged = bytes.clone();
            damaged[index] = value;
            assert!(AudioReference::from_bytes(&damaged).is_err());
        }
        let mut damaged = bytes;
        damaged[AudioReference::HEADER_BYTES..AudioReference::HEADER_BYTES + 4]
            .copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(AudioReference::from_bytes(&damaged).is_err());
    }
}
