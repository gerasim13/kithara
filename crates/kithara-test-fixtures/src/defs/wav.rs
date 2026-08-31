use kithara_test_macros as kithara;
use num_traits::cast;

use crate::signal::{Pcm, Wave, header, wav, wav_from_fn};

struct Consts;

impl Consts {
    const BEAT_MARKER_PEAK: i16 = 22_000;
    const BEAT_TONE_PEAK: i16 = 10_000;
    const BEATS_PER_BAR: usize = 4;
    const CHANNELS: u16 = 2;
    const DOWNBEAT_MARKER_PEAK: i16 = 28_000;
    const DOWNBEAT_TONE_PEAK: i16 = 14_000;
    const MARKER_FRAMES: usize = 2_205;
    const MARKER_PEAK: i16 = 2_000;
    const MARKER_STARTS: [usize; 2] = [17_640, 35_280];
    const MILLIS_PER_SECOND: usize = 1_000;
    const PULSE_DURATION_MS: usize = 40;
    const SAMPLE_RATE: u32 = 44_100;
    const SECONDS_PER_MINUTE: f64 = 60.0;
    const SOURCE_FRAMES: usize = 264_600;
    const TONE_HZ: f64 = 440.0;
    const TONE_PEAK: i16 = 16_000;
}

#[derive(Clone, Copy)]
pub(super) enum RhythmControl {
    Aligned,
    BarPhaseBeats(usize),
    MissingBeat(usize),
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
#[case::deck_a_120bpm_48k(48_000, 2, 576_000, 120.0, 220.0, 0, RhythmControl::Aligned)]
#[case::deck_b_120bpm_48k(48_000, 2, 576_000, 120.0, 880.0, 0, RhythmControl::Aligned)]
#[case::deck_c_120bpm_48k(48_000, 2, 576_000, 120.0, 1_760.0, 0, RhythmControl::Aligned)]
#[case::deck_d_120bpm_48k(48_000, 2, 576_000, 120.0, 3_520.0, 0, RhythmControl::Aligned)]
#[case::deck_b_one_frame_late_120bpm_48k(
    48_000,
    2,
    576_000,
    120.0,
    880.0,
    1,
    RhythmControl::Aligned
)]
#[case::deck_b_one_beat_bar_late_120bpm_48k(
    48_000,
    2,
    576_000,
    120.0,
    880.0,
    0,
    RhythmControl::BarPhaseBeats(1)
)]
#[case::deck_b_missing_beat_120bpm_48k(
    48_000,
    2,
    576_000,
    120.0,
    880.0,
    0,
    RhythmControl::MissingBeat(5)
)]
fn rhythm_wav(
    sample_rate: u32,
    channels: u16,
    total_frames: usize,
    bpm: f64,
    carrier_hz: f64,
    phase_frame: usize,
    control: RhythmControl,
) -> Vec<u8> {
    let mut bytes = header(
        sample_rate,
        channels,
        Some(total_frames * usize::from(channels) * size_of::<i16>()),
    );
    bytes.extend(Vec::<u8>::from(rhythm_pcm(
        sample_rate,
        channels,
        total_frames,
        bpm,
        carrier_hz,
        phase_frame,
        control,
    )));
    bytes
}

pub(super) fn rhythm_pcm(
    sample_rate: u32,
    channels: u16,
    total_frames: usize,
    bpm: f64,
    carrier_hz: f64,
    phase_frame: usize,
    control: RhythmControl,
) -> Pcm {
    let beat_frames: usize =
        cast((f64::from(sample_rate) * Consts::SECONDS_PER_MINUTE / bpm).round())
            .expect("invariant: a fixture beat period fits usize");
    let first_beat = beat_frames + phase_frame;
    let pulse_frames = usize::try_from(sample_rate).expect("invariant: a sample rate fits usize")
        * Consts::PULSE_DURATION_MS
        / Consts::MILLIS_PER_SECOND;
    let (bar_phase, missing_beat) = match control {
        RhythmControl::Aligned => (0, None),
        RhythmControl::BarPhaseBeats(phase) => (phase, None),
        RhythmControl::MissingBeat(missing) => (0, Some(missing)),
    };

    Pcm::from_fn(sample_rate, channels, total_frames, |frame| {
        let Some(since_first) = frame.checked_sub(first_beat) else {
            return 0;
        };
        let beat = since_first / beat_frames;
        let within_beat = since_first % beat_frames;
        if missing_beat == Some(beat) {
            return 0;
        }
        let downbeat = beat % Consts::BEATS_PER_BAR == bar_phase;
        if within_beat == 0 {
            return if downbeat {
                Consts::DOWNBEAT_MARKER_PEAK
            } else {
                Consts::BEAT_MARKER_PEAK
            };
        }
        if within_beat >= pulse_frames {
            return 0;
        }

        Wave::Sine {
            hz: carrier_hz,
            peak: if downbeat {
                Consts::DOWNBEAT_TONE_PEAK
            } else {
                Consts::BEAT_TONE_PEAK
            },
        }
        .sample(within_beat, sample_rate)
    })
}
