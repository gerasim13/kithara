use kithara_test_macros as kithara;
use num_traits::cast;

struct Consts;

impl Consts {
    const BITS_PER_SAMPLE: u16 = 16;
    const CHANNELS: u16 = 2;
    const FMT_CHUNK_BYTES: u32 = 16;
    const HEADER_BYTES: usize = 44;
    const MARKER_FRAMES: usize = 2_205;
    const MARKER_PEAK: i16 = 2_000;
    const MARKER_STARTS: [usize; 2] = [17_640, 35_280];
    const PCM_FORMAT_TAG: u16 = 1;
    const RIFF_PRELUDE_BYTES: usize = 8;
    const SAMPLE_BYTES: usize = 2;
    const SAMPLE_RATE: u32 = 44_100;
    const SOURCE_FRAMES: usize = 264_600;
    const TONE_HZ: f64 = 440.0;
    const TONE_PEAK: i16 = 16_000;
}

/// Interleaved 16-bit RIFF/WAVE around a per-frame sample function.
fn wav(total_frames: usize, sample: impl Fn(usize) -> i16) -> Vec<u8> {
    let channels = usize::from(Consts::CHANNELS);
    let data_bytes = total_frames * channels * Consts::SAMPLE_BYTES;
    let mut out = Vec::with_capacity(Consts::HEADER_BYTES + data_bytes);

    let block_align =
        Consts::CHANNELS * u16::try_from(Consts::SAMPLE_BYTES).expect("invariant: 2 fits u16");
    let byte_rate = Consts::SAMPLE_RATE * u32::from(block_align);
    let riff_bytes = u32::try_from(Consts::HEADER_BYTES - Consts::RIFF_PRELUDE_BYTES + data_bytes)
        .expect("invariant: a fixture WAV fits u32");

    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_bytes.to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&Consts::FMT_CHUNK_BYTES.to_le_bytes());
    out.extend_from_slice(&Consts::PCM_FORMAT_TAG.to_le_bytes());
    out.extend_from_slice(&Consts::CHANNELS.to_le_bytes());
    out.extend_from_slice(&Consts::SAMPLE_RATE.to_le_bytes());
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
        for _ in 0..channels {
            out.extend_from_slice(&value);
        }
    }
    out
}

fn sine(frame: usize, peak: i16) -> i16 {
    let frame = f64::from(u32::try_from(frame).expect("invariant: a fixture is under 2^32 frames"));
    let phase = std::f64::consts::TAU * Consts::TONE_HZ * frame / f64::from(Consts::SAMPLE_RATE);
    let scaled = (phase.sin() * f64::from(peak))
        .round()
        .clamp(f64::from(i16::MIN), f64::from(i16::MAX));
    cast(scaled).unwrap_or(0)
}

/// Plain 440 Hz tone.
#[kithara::asset(ext = "wav", content_type = "audio/wav")]
#[case::a440_6s(Consts::SOURCE_FRAMES, Consts::TONE_PEAK)]
fn sine_wav(total_frames: usize, peak: i16) -> Vec<u8> {
    wav(total_frames, |frame| sine(frame, peak))
}

/// 440 Hz tone with two lower-amplitude source-time markers.
#[kithara::asset(ext = "wav", content_type = "audio/wav")]
#[case::a440_6s(Consts::SOURCE_FRAMES, Consts::TONE_PEAK, Consts::MARKER_PEAK)]
fn marked_sine_wav(total_frames: usize, peak: i16, marker_peak: i16) -> Vec<u8> {
    wav(total_frames, |frame| {
        let in_marker = Consts::MARKER_STARTS
            .iter()
            .any(|start| frame >= *start && frame < start + Consts::MARKER_FRAMES);
        sine(frame, if in_marker { marker_peak } else { peak })
    })
}
