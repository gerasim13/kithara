use kithara_test_macros as kithara;
use num_traits::cast;

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

/// Four-beat pulse track with an exact frame-addressable beat marker.
#[kithara::asset(ext = "wav", content_type = "audio/wav")]
#[case::deck_a_120bpm_48k(48_000, 2, 576_000, 120.0, 220.0, 0)]
#[case::deck_b_120bpm_48k(48_000, 2, 576_000, 120.0, 880.0, 0)]
#[case::deck_c_120bpm_48k(48_000, 2, 576_000, 120.0, 1_760.0, 0)]
#[case::deck_d_120bpm_48k(48_000, 2, 576_000, 120.0, 3_520.0, 0)]
#[case::deck_b_one_frame_late_120bpm_48k(48_000, 2, 576_000, 120.0, 880.0, 1)]
fn rhythm_wav(
    sample_rate: u32,
    channels: u16,
    total_frames: usize,
    bpm: f64,
    carrier_hz: f64,
    phase_frame: usize,
) -> Vec<u8> {
    let beat_frames: usize = cast((f64::from(sample_rate) * 60.0 / bpm).round())
        .expect("invariant: a fixture beat period fits usize");
    let first_beat = beat_frames + phase_frame;
    let pulse_frames =
        usize::try_from(sample_rate).expect("invariant: a sample rate fits usize") / 25;

    wav_from_fn(sample_rate, channels, total_frames, |frame| {
        let Some(since_first) = frame.checked_sub(first_beat) else {
            return 0;
        };
        let beat = since_first / beat_frames;
        let within_beat = since_first % beat_frames;
        if within_beat == 0 {
            return if beat.is_multiple_of(4) {
                28_000
            } else {
                22_000
            };
        }
        if within_beat >= pulse_frames {
            return 0;
        }

        Wave::Sine {
            hz: carrier_hz,
            peak: if beat.is_multiple_of(4) {
                14_000
            } else {
                10_000
            },
        }
        .sample(within_beat, sample_rate)
    })
}
