use kithara_encode::PcmSource;

/// Interleaved 16-bit PCM held in memory for an encoder to pull from.
pub(super) struct Pcm {
    bytes: Vec<u8>,
    channels: u16,
    sample_rate: u32,
}

impl Pcm {
    pub(super) fn new(
        sample_rate: u32,
        channels: u16,
        total_frames: usize,
        sample: impl Fn(usize) -> i16,
    ) -> Self {
        let lanes = usize::from(channels);
        let mut bytes = Vec::with_capacity(total_frames * lanes * size_of::<i16>());
        for frame in 0..total_frames {
            let value = sample(frame).to_le_bytes();
            for _ in 0..lanes {
                bytes.extend_from_slice(&value);
            }
        }
        Self {
            bytes,
            channels,
            sample_rate,
        }
    }
}

impl PcmSource for Pcm {
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
