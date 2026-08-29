use kithara_encode::{BytesEncodeRequest, BytesEncodeTarget, EncoderFactory, PcmSource};
use kithara_test_macros as kithara;

use super::tone;

struct Consts;

impl Consts {
    const CHANNELS: u16 = 2;
    const SAMPLE_BYTES: usize = 2;
    const SAMPLE_RATE: u32 = 44_100;
    const TONE_HZ: f64 = 440.0;
    const TONE_PEAK: i16 = 16_000;
    const TWO_SECOND_FRAMES: usize = 88_200;
}

/// Interleaved 16-bit sine, held in memory for the encoder to pull from.
struct SinePcm {
    bytes: Vec<u8>,
}

impl SinePcm {
    fn new(total_frames: usize, peak: i16) -> Self {
        let channels = usize::from(Consts::CHANNELS);
        let mut bytes = Vec::with_capacity(total_frames * channels * Consts::SAMPLE_BYTES);
        for frame in 0..total_frames {
            let sample =
                tone::sine(frame, Consts::SAMPLE_RATE, Consts::TONE_HZ, peak).to_le_bytes();
            for _ in 0..channels {
                bytes.extend_from_slice(&sample);
            }
        }
        Self { bytes }
    }
}

impl PcmSource for SinePcm {
    fn channels(&self) -> u16 {
        Consts::CHANNELS
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
        Consts::SAMPLE_RATE
    }

    fn total_byte_len(&self) -> Option<usize> {
        Some(self.bytes.len())
    }
}

/// 440 Hz tone encoded to MPEG audio.
#[kithara::asset(ext = "mp3", content_type = "audio/mpeg")]
#[case::a440_2s(Consts::TWO_SECOND_FRAMES, Consts::TONE_PEAK)]
fn sine_mp3(total_frames: usize, peak: i16) -> Vec<u8> {
    let pcm = SinePcm::new(total_frames, peak);
    EncoderFactory::encode_bytes(BytesEncodeRequest {
        pcm: &pcm,
        target: BytesEncodeTarget::Mp3,
        bit_rate: None,
    })
    .unwrap_or_else(|error| panic!("kithara-fixtures: MP3 encode failed: {error}"))
    .bytes
}
