use std::f64::consts::TAU;

use kithara_analysis::BeatArtifact;
use num_traits::cast;

use crate::signal::{Pcm, header};

struct Consts;

impl Consts {
    const BEATS_PER_BAR: usize = 4;
    const CHANNELS: u16 = 2;
    const MISSING_BEAT: usize = 5;
    const SAMPLE_RATE: u32 = 48_000;
    const SECONDS: usize = 12;
    const SECONDS_PER_MINUTE: f64 = 60.0;
    const TOTAL_FRAMES: usize = 48_000 * Self::SECONDS;
}

#[derive(Clone, Copy)]
pub(super) enum Control {
    Aligned,
    OneFrameLate,
    OneBeatBarLate,
    MissingBeat,
}

#[derive(Clone, Copy)]
pub(super) enum Style {
    AmbientDub,
    TripHop,
    Downtempo,
    House,
    Techno,
    Breakbeat,
}

impl Style {
    pub(super) const fn bpm(self) -> f64 {
        match self {
            Self::AmbientDub => 62.0,
            Self::TripHop => 74.0,
            Self::Downtempo => 96.0,
            Self::House => 124.0,
            Self::Techno => 132.0,
            Self::Breakbeat => 140.0,
        }
    }

    const fn bass_hz(self) -> f64 {
        match self {
            Self::AmbientDub => 55.0,
            Self::TripHop => 65.41,
            Self::Downtempo => 73.42,
            Self::House => 82.41,
            Self::Techno => 92.50,
            Self::Breakbeat => 110.0,
        }
    }

    const fn hat_divisions(self) -> usize {
        match self {
            Self::AmbientDub => 1,
            Self::TripHop | Self::Downtempo | Self::House => 2,
            Self::Techno | Self::Breakbeat => 4,
        }
    }

    const fn kick(self, beat: usize) -> bool {
        match self {
            Self::AmbientDub => matches!(beat % 4, 0 | 3),
            Self::TripHop => matches!(beat % 8, 0 | 3 | 6),
            Self::Downtempo => matches!(beat % 4, 0 | 2),
            Self::House | Self::Techno => true,
            Self::Breakbeat => matches!(beat % 8, 0 | 2 | 5 | 7),
        }
    }
}

pub(super) struct Truth {
    bpm: f64,
    beats: Vec<(u64, Option<f32>)>,
    downbeats: Vec<(u64, Option<f32>)>,
}

impl From<Truth> for BeatArtifact {
    fn from(truth: Truth) -> Self {
        Self::new(truth.bpm, truth.beats, truth.downbeats)
    }
}

pub(super) fn wav(style: Style, control: Control) -> Vec<u8> {
    let pcm = pcm(style, control);
    let mut bytes = header(
        Consts::SAMPLE_RATE,
        Consts::CHANNELS,
        Some(Consts::TOTAL_FRAMES * usize::from(Consts::CHANNELS) * size_of::<i16>()),
    );
    bytes.extend(Vec::<u8>::from(pcm));
    bytes
}

pub(super) fn truth(style: Style, control: Control) -> Truth {
    let beat_frames = beat_frames(style);
    let first = beat_frames + phase_frames(control);
    let bar_phase = usize::from(matches!(control, Control::OneBeatBarLate));
    let mut beats = Vec::new();
    let mut downbeats = Vec::new();
    for (beat, frame) in (first..Consts::TOTAL_FRAMES)
        .step_by(beat_frames)
        .enumerate()
    {
        if matches!(control, Control::MissingBeat) && beat == Consts::MISSING_BEAT {
            continue;
        }
        let mark = (u64::try_from(frame).unwrap_or(u64::MAX), Some(1.0));
        beats.push(mark);
        if beat % Consts::BEATS_PER_BAR == bar_phase {
            downbeats.push(mark);
        }
    }
    Truth {
        bpm: style.bpm(),
        beats,
        downbeats,
    }
}

fn pcm(style: Style, control: Control) -> Pcm {
    let beat_frames = beat_frames(style);
    let first = beat_frames + phase_frames(control);
    Pcm::from_fn(
        Consts::SAMPLE_RATE,
        Consts::CHANNELS,
        Consts::TOTAL_FRAMES,
        |frame| sample(style, control, frame, first, beat_frames),
    )
}

fn beat_frames(style: Style) -> usize {
    cast((f64::from(Consts::SAMPLE_RATE) * Consts::SECONDS_PER_MINUTE / style.bpm()).round())
        .expect("invariant: a rhythm beat period fits usize")
}

const fn phase_frames(control: Control) -> usize {
    match control {
        Control::OneFrameLate => 1,
        Control::Aligned | Control::OneBeatBarLate | Control::MissingBeat => 0,
    }
}

fn sample(style: Style, control: Control, frame: usize, first: usize, beat_frames: usize) -> i16 {
    let Some(since_first) = frame.checked_sub(first) else {
        return pad(style, frame);
    };
    let beat = since_first / beat_frames;
    let within = since_first % beat_frames;
    let missing = matches!(control, Control::MissingBeat) && beat == Consts::MISSING_BEAT;
    if missing {
        return pad(style, frame);
    }
    let bar_phase = usize::from(matches!(control, Control::OneBeatBarLate));
    let pattern_beat = beat + Consts::BEATS_PER_BAR - bar_phase;
    if within == 0 {
        return if pattern_beat.is_multiple_of(Consts::BEATS_PER_BAR) {
            28_000
        } else {
            22_000
        };
    }

    let kick = if style.kick(pattern_beat) {
        decaying_sine(within, 0.12, 52.0, 11_000.0)
    } else {
        0.0
    };
    let snare = if pattern_beat % Consts::BEATS_PER_BAR == 1
        || pattern_beat % Consts::BEATS_PER_BAR == 3
    {
        decaying_sine(within, 0.08, 190.0, 4_000.0) + decaying_sine(within, 0.05, 2_900.0, 2_000.0)
    } else {
        0.0
    };
    let division_frames = beat_frames / style.hat_divisions();
    let hat_within = within % division_frames.max(1);
    let hat = decaying_sine(hat_within, 0.025, 7_200.0, 1_400.0);
    let bass = decaying_sine(within, 0.32, style.bass_hz(), 3_600.0);
    let pad = f64::from(pad(style, frame));
    cast::<_, i16>(
        (kick + snare + hat + bass + pad)
            .clamp(-30_000.0, 30_000.0)
            .round(),
    )
    .unwrap_or(0)
}

fn decaying_sine(frame: usize, seconds: f64, hz: f64, peak: f64) -> f64 {
    let elapsed = frame_seconds(frame);
    if elapsed >= seconds {
        return 0.0;
    }
    let envelope = 1.0 - elapsed / seconds;
    (TAU * hz * elapsed).sin() * peak * envelope * envelope
}

fn pad(style: Style, frame: usize) -> i16 {
    let elapsed = frame_seconds(frame);
    let sample = (TAU * style.bass_hz() * 2.0 * elapsed).sin() * 500.0
        + (TAU * style.bass_hz() * 3.0 * elapsed).sin() * 250.0;
    cast(sample.round()).unwrap_or(0)
}

fn frame_seconds(frame: usize) -> f64 {
    let frame = cast::<_, f64>(frame).expect("invariant: generated frame index fits f64 exactly");
    frame / f64::from(Consts::SAMPLE_RATE)
}
