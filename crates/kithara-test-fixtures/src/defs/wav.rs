use kithara_test_macros as kithara;

use crate::signal::{Wave, wav, wav_from_fn};

struct Consts;

impl Consts {
    const CHANNELS: u16 = 2;
    const MARKER_FRAMES: usize = 2_205;
    const MARKER_PEAK: i16 = 2_000;
    const MARKER_STARTS: [usize; 2] = [17_640, 35_280];
    const SAMPLE_RATE: u32 = 44_100;
    const SOURCE_FRAMES: usize = 264_600;
    const TONE_HZ: f64 = 440.0;
    const TONE_PEAK: i16 = 16_000;
}

/// Plain 440 Hz tone.
#[kithara::asset(ext = "wav", content_type = "audio/wav")]
#[case::a440_6s(Consts::SOURCE_FRAMES, Consts::TONE_PEAK)]
fn sine_wav(total_frames: usize, peak: i16) -> Vec<u8> {
    wav(
        Consts::SAMPLE_RATE,
        Consts::CHANNELS,
        total_frames,
        Wave::Sine {
            hz: Consts::TONE_HZ,
            peak,
        },
    )
}

/// 440 Hz tone with two lower-amplitude source-time markers.
#[kithara::asset(ext = "wav", content_type = "audio/wav", embed)]
#[case::a440_6s(Consts::SOURCE_FRAMES, Consts::TONE_PEAK, Consts::MARKER_PEAK)]
fn marked_sine_wav(total_frames: usize, peak: i16, marker_peak: i16) -> Vec<u8> {
    wav_from_fn(
        Consts::SAMPLE_RATE,
        Consts::CHANNELS,
        total_frames,
        |frame| {
            let in_marker = Consts::MARKER_STARTS
                .iter()
                .any(|start| frame >= *start && frame < start + Consts::MARKER_FRAMES);
            let peak = if in_marker { marker_peak } else { peak };
            Wave::Sine {
                hz: Consts::TONE_HZ,
                peak,
            }
            .sample(frame, Consts::SAMPLE_RATE)
        },
    )
}
