use std::sync::OnceLock;

use cochlea_features::{
    Audio, ProbeOpts, SegmentOpts, TempoOpts, estimate_tempo, probe, segment_timeline,
};
use serde::Serialize;

pub(crate) const DEFAULT_WINDOW_MS: f64 = 5.0;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[non_exhaustive]
pub struct CochleaReport {
    pub silent_segments: usize,
    pub onset_times_ms: Vec<f64>,
    pub beat_times_ms: Vec<f64>,
    pub tempo_bpm: Option<f64>,
    pub tempo_confidence: f64,
    pub clear_rhythm: bool,
    pub clipped_samples: usize,
    pub true_peak_over_0dbtp: bool,
    pub leading_silence_ms: f64,
    pub trailing_silence_ms: f64,
}

impl CochleaReport {
    #[must_use]
    pub fn measure(samples: &[f32], channels: u16, sample_rate: u32) -> Self {
        let audio = Audio {
            samples: samples.to_vec(),
            channels,
            sample_rate,
        };
        let report = probe(&audio, &ProbeOpts::default());
        let tempo = estimate_tempo(&audio, &TempoOpts::default());
        let silent_segments = segment_timeline(
            &audio,
            &SegmentOpts::default().with_window_ms(DEFAULT_WINDOW_MS),
        )
        .segments
        .iter()
        .filter(|segment| segment.silent)
        .count();

        Self {
            silent_segments,
            onset_times_ms: report.onsets.times_ms,
            beat_times_ms: tempo.beats_ms,
            tempo_bpm: tempo.bpm,
            tempo_confidence: tempo.confidence,
            clear_rhythm: tempo.clear_rhythm,
            clipped_samples: report.clipping.clipped_samples,
            true_peak_over_0dbtp: report.clipping.true_peak_over_0dbtp,
            leading_silence_ms: report.silence.leading_ms,
            trailing_silence_ms: report.silence.trailing_ms,
        }
    }

    #[must_use]
    pub fn onset_count(&self) -> usize {
        self.onset_times_ms.len()
    }
}

#[must_use]
pub fn continuity_failures(
    label: &str,
    candidate: &CochleaReport,
    control: &CochleaReport,
) -> Vec<String> {
    let mut failures = Vec::new();
    if candidate.silent_segments > control.silent_segments {
        failures.push(format!(
            "{label}: Cochlea found extra silent segments: candidate={}, control={}",
            candidate.silent_segments, control.silent_segments,
        ));
    }
    if candidate.onset_count() > control.onset_count() {
        failures.push(format!(
            "{label}: Cochlea found extra onsets: candidate={}, control={}, times_ms={:?}",
            candidate.onset_count(),
            control.onset_count(),
            candidate.onset_times_ms,
        ));
    } else if candidate.onset_count() < control.onset_count() {
        failures.push(format!(
            "{label}: Cochlea found missing onsets: candidate={}, control={}, control_times_ms={:?}",
            candidate.onset_count(),
            control.onset_count(),
            control.onset_times_ms,
        ));
    }
    if candidate.clipped_samples > control.clipped_samples {
        failures.push(format!(
            "{label}: Cochlea found extra clipped samples: candidate={}, control={}",
            candidate.clipped_samples, control.clipped_samples,
        ));
    }
    if candidate.true_peak_over_0dbtp && !control.true_peak_over_0dbtp {
        failures.push(format!(
            "{label}: Cochlea found a candidate-only true peak over 0 dBTP"
        ));
    }
    if candidate.leading_silence_ms > control.leading_silence_ms + DEFAULT_WINDOW_MS {
        failures.push(format!(
            "{label}: Cochlea found extra leading silence: candidate={:.3}ms, control={:.3}ms",
            candidate.leading_silence_ms, control.leading_silence_ms,
        ));
    }
    if candidate.trailing_silence_ms > control.trailing_silence_ms + DEFAULT_WINDOW_MS {
        failures.push(format!(
            "{label}: Cochlea found extra trailing silence: candidate={:.3}ms, control={:.3}ms",
            candidate.trailing_silence_ms, control.trailing_silence_ms,
        ));
    }
    failures
}

pub fn assert_oracle_load_bearing(
    control: &[f32],
    channels: u16,
    sample_rate: u32,
    quantum_frames: usize,
) {
    let channel_count = channels;
    let channels = usize::from(channel_count);
    assert!(channels > 0, "Cochlea oracle needs at least one channel");
    let control_audio = Audio {
        samples: control.to_vec(),
        channels: channel_count,
        sample_rate,
    };
    let control_probe = probe(&control_audio, &ProbeOpts::default());
    assert_eq!(
        control_probe.clipping.clipped_samples, 0,
        "Cochlea oracle control must not already be clipped"
    );

    let middle_frame = control.len() / channels / 2;
    let gap_start_frame = middle_frame.saturating_sub(quantum_frames / 2);
    let gap_end_frame = gap_start_frame.saturating_add(quantum_frames);
    assert!(
        gap_end_frame * channels <= control.len(),
        "Cochlea oracle control is too short for one injected quantum"
    );
    let mut gapped = control.to_vec();
    gapped[gap_start_frame * channels..gap_end_frame * channels].fill(0.0);
    let control_silence = silent_segment_count(&control_audio);
    let gap_audio = Audio {
        samples: gapped,
        channels: channel_count,
        sample_rate,
    };
    let gap_silence = silent_segment_count(&gap_audio);
    assert!(
        gap_silence > control_silence,
        "Cochlea oracle did not reject one missing quantum: control={control_silence}, gapped={gap_silence}"
    );

    let mut clicked = control.to_vec();
    let click_frame = middle_frame.saturating_add(17);
    clicked[click_frame * channels..(click_frame + 1) * channels].fill(1.0);
    let click_audio = Audio {
        samples: clicked,
        channels: channel_count,
        sample_rate,
    };
    let click_probe = probe(&click_audio, &ProbeOpts::default());
    assert_eq!(
        click_probe.clipping.clipped_samples,
        control_probe.clipping.clipped_samples + channels,
        "Cochlea oracle did not count one injected clipped frame"
    );

    assert_phase_oracle_load_bearing();
}

pub(crate) fn assert_rhythmic_oracle_load_bearing() {
    const CHANNELS: usize = 2;
    const QUANTUM_FRAMES: usize = 512;
    const SAMPLE_RATE: u32 = 48_000;

    let control = rhythmic_calibration(CHANNELS, SAMPLE_RATE);
    assert_oracle_load_bearing(&control, CHANNELS as u16, SAMPLE_RATE, QUANTUM_FRAMES);
}

fn assert_phase_oracle_load_bearing() {
    const CHANNELS: usize = 2;
    const QUANTUM_FRAMES: usize = 512;
    const SAMPLE_RATE: u32 = 48_000;

    static CALIBRATED: OnceLock<()> = OnceLock::new();
    CALIBRATED.get_or_init(|| {
        let rhythmic = rhythmic_calibration(CHANNELS, SAMPLE_RATE);
        let rhythmic_audio = Audio {
            samples: rhythmic.clone(),
            channels: CHANNELS as u16,
            sample_rate: SAMPLE_RATE,
        };
        let rhythmic_report = estimate_tempo(&rhythmic_audio, &TempoOpts::default());
        assert!(
            rhythmic_report.clear_rhythm && rhythmic_report.beats_ms.len() >= 3,
            "Cochlea phase oracle needs a clear rhythmic calibration signal: {rhythmic_report:?}"
        );
        let shift_samples = QUANTUM_FRAMES.saturating_mul(CHANNELS);
        assert!(
            shift_samples < rhythmic.len(),
            "Cochlea oracle control is too short for the phase-shift injection"
        );
        let mut shifted = vec![0.0; shift_samples];
        shifted.extend_from_slice(&rhythmic[..rhythmic.len() - shift_samples]);
        let shifted_audio = Audio {
            samples: shifted,
            channels: CHANNELS as u16,
            sample_rate: SAMPLE_RATE,
        };
        let shifted_report = estimate_tempo(&shifted_audio, &TempoOpts::default());
        let compared = rhythmic_report
            .beats_ms
            .iter()
            .zip(&shifted_report.beats_ms)
            .take(8)
            .map(|(control, shifted)| (shifted - control).abs())
            .collect::<Vec<_>>();
        let expected_shift_ms =
            QUANTUM_FRAMES as f64 / f64::from(SAMPLE_RATE) * 1_000.0;
        assert!(
            compared.len() >= 3
                && compared
                    .iter()
                    .filter(|offset| **offset >= expected_shift_ms / 2.0)
                    .count()
                    >= compared.len() / 2,
            "Cochlea oracle did not reject a {QUANTUM_FRAMES}-frame phase shift: expected about {expected_shift_ms:.3}ms, offsets={compared:?}"
        );
    });
}

fn silent_segment_count(audio: &Audio) -> usize {
    segment_timeline(
        audio,
        &SegmentOpts::default().with_window_ms(DEFAULT_WINDOW_MS),
    )
    .segments
    .iter()
    .filter(|segment| segment.silent)
    .count()
}

pub(crate) fn rhythmic_calibration(channels: usize, sample_rate: u32) -> Vec<f32> {
    const BPM: f64 = 120.0;
    const SECONDS: usize = 12;
    const TONE_HZ: f64 = 880.0;

    let frames = sample_rate as usize * SECONDS;
    let beat_frames = (f64::from(sample_rate) * 60.0 / BPM).round() as usize;
    let burst_frames = beat_frames / 10;
    let mut pcm = Vec::with_capacity(frames.saturating_mul(channels));
    for frame in 0..frames {
        let into_beat = frame % beat_frames;
        let sample = if frame >= beat_frames && into_beat < burst_frames {
            let decay = 1.0 - into_beat as f64 / burst_frames as f64;
            let phase = std::f64::consts::TAU * TONE_HZ * into_beat as f64 / f64::from(sample_rate);
            (phase.sin() * decay * decay * 0.6) as f32
        } else {
            0.0
        };
        pcm.extend(std::iter::repeat_n(sample, channels));
    }
    pcm
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cochlea_oracle_rejects_gap_clip_and_phase_injections() {
        assert_rhythmic_oracle_load_bearing();
    }

    #[test]
    fn shared_continuity_comparison_rejects_missing_onsets() {
        let control = CochleaReport {
            silent_segments: 0,
            onset_times_ms: vec![500.0, 1_000.0, 1_500.0],
            beat_times_ms: vec![500.0, 1_000.0, 1_500.0],
            tempo_bpm: Some(120.0),
            tempo_confidence: 1.0,
            clear_rhythm: true,
            clipped_samples: 0,
            true_peak_over_0dbtp: false,
            leading_silence_ms: 0.0,
            trailing_silence_ms: 0.0,
        };
        let mut candidate = control.clone();
        candidate.onset_times_ms.remove(1);

        let failures = continuity_failures("missing event", &candidate, &control);
        assert!(
            failures
                .iter()
                .any(|failure| failure.contains("missing onsets")),
            "shared continuity comparison accepted a missing onset: {failures:?}"
        );
    }
}
