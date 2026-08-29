struct Consts;

impl Consts {
    const BITS_PER_SAMPLE: u16 = 16;
    const FMT_CHUNK_BYTES: u32 = 16;
    const HEADER_BYTES: usize = 44;
    const PCM_FORMAT_TAG: u16 = 1;
    const RIFF_PRELUDE_BYTES: usize = 8;
    const SAMPLE_BYTES: usize = 2;
}

/// Interleaved 16-bit RIFF/WAVE around a per-frame sample function.
pub(super) fn wav(
    sample_rate: u32,
    channels: u16,
    total_frames: usize,
    sample: impl Fn(usize) -> i16,
) -> Vec<u8> {
    let data_bytes = total_frames * usize::from(channels) * Consts::SAMPLE_BYTES;
    let mut out = Vec::with_capacity(Consts::HEADER_BYTES + data_bytes);

    let block_align =
        channels * u16::try_from(Consts::SAMPLE_BYTES).expect("invariant: 2 fits u16");
    let byte_rate = sample_rate * u32::from(block_align);
    let riff_bytes = u32::try_from(Consts::HEADER_BYTES - Consts::RIFF_PRELUDE_BYTES + data_bytes)
        .expect("invariant: a fixture WAV fits u32");

    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_bytes.to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&Consts::FMT_CHUNK_BYTES.to_le_bytes());
    out.extend_from_slice(&Consts::PCM_FORMAT_TAG.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&Consts::BITS_PER_SAMPLE.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(
        &u32::try_from(data_bytes)
            .expect("invariant: a fixture WAV payload fits u32")
            .to_le_bytes(),
    );

    for frame in 0..total_frames {
        let value = sample(frame).to_le_bytes();
        for _ in 0..usize::from(channels) {
            out.extend_from_slice(&value);
        }
    }
    out
}
