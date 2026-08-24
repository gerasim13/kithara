use std::collections::BTreeMap;

use super::{
    COCHLEA_PHASE_SPREAD_BUDGET_FRAMES, LockedPhaseObservation, MAX_LOCKED_PHASE_ERROR_FRAMES,
    PcmCapture, SignalEvidence, SyncCase,
};
use crate::cochlea::{CochleaReport, DEFAULT_WINDOW_MS};

const MIN_MATCHED_BEATS: usize = 3;
pub(super) const COCHLEA_PHASE_CALIBRATION_SHIFT_FRAMES: usize = 512;
const COCHLEA_ONSET_HOP_FRAMES: f64 = 256.0;
const POST_SYNC_TEMPO_BIN_TOLERANCE_BPM: f64 = 0.05;

/// Stable category for one shared signal-oracle rejection.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum SignalFailureKind {
    ActivationFrame,
    BarPhase,
    BeatOrdinal,
    Continuity,
    EmptyPcm,
    LockedPhase,
    MapIdentity,
    MissingPhaseObservation,
    PostSyncPhase,
    PostSyncPhaseSpread,
    PostSyncTempo,
    PreSyncPhase,
    RhythmicEventDivergence,
    RhythmicEventLoss,
}

/// One typed signal-oracle rejection with its diagnostic message.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct SignalFailure {
    kind: SignalFailureKind,
    message: String,
}

impl SignalFailure {
    fn new(kind: SignalFailureKind, message: String) -> Self {
        Self { kind, message }
    }

    #[must_use]
    pub const fn kind(&self) -> SignalFailureKind {
        self.kind
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Result of evaluating provider-independent sync signal evidence.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct SignalOracleReport {
    failures: Vec<SignalFailure>,
    post_sync_spread_frames: Option<u64>,
    pre_sync_spread_frames: Option<u64>,
}

impl SignalOracleReport {
    #[must_use]
    pub fn is_success(&self) -> bool {
        self.failures.is_empty()
    }

    #[must_use]
    pub fn failures(&self) -> &[SignalFailure] {
        &self.failures
    }

    pub fn failure_kinds(&self) -> impl Iterator<Item = SignalFailureKind> + '_ {
        self.failures.iter().map(SignalFailure::kind)
    }

    pub(super) fn into_runtime_parts(self) -> (Vec<String>, Option<u64>, Option<u64>) {
        let failures = self
            .failures
            .into_iter()
            .map(|failure| failure.message)
            .collect();
        (
            failures,
            self.post_sync_spread_frames,
            self.pre_sync_spread_frames,
        )
    }
}

/// Evaluates signal evidence shared by asset and Player/Queue providers.
#[derive(Debug)]
pub struct SignalOracle;

impl SignalOracle {
    #[must_use]
    pub fn evaluate(case: SyncCase, evidence: &SignalEvidence) -> SignalOracleReport {
        let cochlea = evidence
            .audio()
            .map(|capture| {
                (
                    capture.label.clone(),
                    CochleaReport::measure(&capture.samples, capture.channels, capture.sample_rate),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mut failures = signal_capture_failures(evidence);
        failures.extend(continuity_failures(evidence, &cochlea));
        failures.extend(exact_phase_failures(&evidence.phase_observations));

        let pre_sync_spread_frames = phase_spread_frames(
            &evidence.pre_sync_replays,
            &cochlea,
            case.session_bpm,
            "pre-SYNC",
            SignalFailureKind::PreSyncPhase,
            &mut failures,
        );
        match pre_sync_spread_frames {
            Some(spread) if spread > COCHLEA_PHASE_SPREAD_BUDGET_FRAMES => {}
            Some(spread) => {
                failures.push(SignalFailure::new(
                    SignalFailureKind::PreSyncPhase,
                    format!(
                        "pre-SYNC Cochlea beat spread is {spread} frames; stagger was not proven beyond the {COCHLEA_PHASE_SPREAD_BUDGET_FRAMES}-frame Cochlea budget"
                    ),
                ));
            }
            None => {}
        }

        let final_bpm = case.tempo_ride.final_bpm();
        post_sync_tempo_failures(&evidence.deck_replays, &cochlea, final_bpm, &mut failures);
        let post_sync_spread_frames = phase_spread_frames(
            &evidence.deck_replays,
            &cochlea,
            final_bpm,
            "post-SYNC",
            SignalFailureKind::PostSyncPhase,
            &mut failures,
        );
        if let Some(spread) = post_sync_spread_frames
            && spread > COCHLEA_PHASE_SPREAD_BUDGET_FRAMES
        {
            failures.push(SignalFailure::new(
                SignalFailureKind::PostSyncPhaseSpread,
                format!(
                    "post-SYNC Cochlea beat spread is {spread} frames; Cochlea budget is {COCHLEA_PHASE_SPREAD_BUDGET_FRAMES} frames"
                ),
            ));
        }

        SignalOracleReport {
            failures,
            post_sync_spread_frames,
            pre_sync_spread_frames,
        }
    }
}

fn signal_capture_failures(evidence: &SignalEvidence) -> Vec<SignalFailure> {
    let mut failures = Vec::new();
    for capture in evidence.audio() {
        if capture.samples.is_empty() {
            failures.push(SignalFailure::new(
                SignalFailureKind::EmptyPcm,
                format!("{}: final rendered PCM is empty", capture.label),
            ));
        }
    }
    failures
}

fn exact_phase_failures(observations: &[LockedPhaseObservation]) -> Vec<SignalFailure> {
    if observations.is_empty() {
        return vec![SignalFailure::new(
            SignalFailureKind::MissingPhaseObservation,
            "post-SYNC: no exact admitted-map phase observations were published".to_owned(),
        )];
    }
    let mut failures = Vec::new();
    for observation in observations {
        if observation.applied_map != observation.admitted_map {
            failures.push(SignalFailure::new(
                SignalFailureKind::MapIdentity,
                format!(
                    "post-SYNC: deck {} applied map {:?}, expected admitted map {:?}",
                    observation.deck, observation.applied_map, observation.admitted_map,
                ),
            ));
        }
        if observation.applied_activation_frame != observation.expected_activation_frame {
            failures.push(SignalFailure::new(
                SignalFailureKind::ActivationFrame,
                format!(
                    "post-SYNC: deck {} activation stamp shifted by {} frame(s): applied={}, expected={}",
                    observation.deck,
                    observation
                        .applied_activation_frame
                        .abs_diff(observation.expected_activation_frame),
                    observation.applied_activation_frame,
                    observation.expected_activation_frame,
                ),
            ));
        }
        let phase_error = observation
            .observed_phase_frame
            .abs_diff(observation.expected_phase_frame);
        if phase_error > MAX_LOCKED_PHASE_ERROR_FRAMES {
            failures.push(SignalFailure::new(
                SignalFailureKind::LockedPhase,
                format!(
                    "post-SYNC: deck {} locked phase error is {phase_error} frames; budget is {MAX_LOCKED_PHASE_ERROR_FRAMES}",
                    observation.deck,
                ),
            ));
        }
        if observation.observed_beat != observation.expected_beat {
            failures.push(SignalFailure::new(
                SignalFailureKind::BeatOrdinal,
                format!(
                    "post-SYNC: deck {} resolved beat ordinal {}, expected {}",
                    observation.deck, observation.observed_beat, observation.expected_beat,
                ),
            ));
        }
        let beats_per_bar = i64::from(observation.meter.beats_per_bar());
        let downbeat = i64::from(observation.meter.downbeat());
        let expected_bar_phase =
            (i64::from(observation.expected_beat) - downbeat).rem_euclid(beats_per_bar);
        let observed_bar_phase =
            (i64::from(observation.observed_beat) - downbeat).rem_euclid(beats_per_bar);
        if observed_bar_phase != expected_bar_phase {
            failures.push(SignalFailure::new(
                SignalFailureKind::BarPhase,
                format!(
                    "post-SYNC: deck {} bar phase is {observed_bar_phase}, expected {expected_bar_phase} in {}",
                    observation.deck, observation.meter,
                ),
            ));
        }
    }
    failures
}

fn continuity_failures(
    evidence: &SignalEvidence,
    reports: &BTreeMap<String, CochleaReport>,
) -> Vec<SignalFailure> {
    let mut failures = Vec::new();
    if let (Some(candidate), Some(control)) = (
        reports.get(&evidence.mix.label),
        reports.get(&evidence.control_mix.label),
    ) {
        failures.extend(compare_continuity(
            "shared lifecycle mix",
            candidate,
            control,
        ));
    }
    for (deck, (candidate, control)) in evidence
        .deck_replays
        .iter()
        .zip(&evidence.control_replays)
        .enumerate()
    {
        if let (Some(candidate), Some(control)) =
            (reports.get(&candidate.label), reports.get(&control.label))
        {
            failures.extend(compare_continuity(
                &format!("deck {deck} final PCM"),
                candidate,
                control,
            ));
        }
    }
    failures
}

fn compare_continuity(
    label: &str,
    candidate: &CochleaReport,
    control: &CochleaReport,
) -> Vec<SignalFailure> {
    let mut failures = Vec::new();
    if candidate.silent_segments > control.silent_segments {
        failures.push(SignalFailure::new(
            SignalFailureKind::Continuity,
            format!(
                "{label}: Cochlea found extra silent segments: candidate={}, control={}",
                candidate.silent_segments, control.silent_segments,
            ),
        ));
    }
    compare_rhythmic_count(
        label,
        "onsets",
        candidate.onset_times_ms.len(),
        control.onset_times_ms.len(),
        &mut failures,
    );
    compare_rhythmic_count(
        label,
        "beats",
        candidate.beat_times_ms.len(),
        control.beat_times_ms.len(),
        &mut failures,
    );
    if candidate.clipped_samples > 0 {
        failures.push(SignalFailure::new(
            SignalFailureKind::Continuity,
            format!(
                "{label}: Cochlea found {} clipped candidate samples; control clipping cannot excuse candidate clipping",
                candidate.clipped_samples
            ),
        ));
    }
    if candidate.true_peak_over_0dbtp {
        failures.push(SignalFailure::new(
            SignalFailureKind::Continuity,
            format!("{label}: Cochlea found a candidate true peak over 0 dBTP"),
        ));
    }
    if control.clipped_samples > 0 || control.true_peak_over_0dbtp {
        failures.push(SignalFailure::new(
            SignalFailureKind::Continuity,
            format!(
                "{label}: conservative free control is not clean: clipped_samples={}, true_peak_over_0dbtp={}",
                control.clipped_samples, control.true_peak_over_0dbtp
            ),
        ));
    }
    if candidate.leading_silence_ms > control.leading_silence_ms + DEFAULT_WINDOW_MS {
        failures.push(SignalFailure::new(
            SignalFailureKind::Continuity,
            format!(
                "{label}: Cochlea found extra leading silence: candidate={:.3}ms, control={:.3}ms",
                candidate.leading_silence_ms, control.leading_silence_ms,
            ),
        ));
    }
    if candidate.trailing_silence_ms > control.trailing_silence_ms + DEFAULT_WINDOW_MS {
        failures.push(SignalFailure::new(
            SignalFailureKind::Continuity,
            format!(
                "{label}: Cochlea found extra trailing silence: candidate={:.3}ms, control={:.3}ms",
                candidate.trailing_silence_ms, control.trailing_silence_ms,
            ),
        ));
    }
    failures
}

fn compare_rhythmic_count(
    label: &str,
    feature: &str,
    candidate: usize,
    control: usize,
    failures: &mut Vec<SignalFailure>,
) {
    if candidate < control {
        failures.push(SignalFailure::new(
            SignalFailureKind::RhythmicEventLoss,
            format!(
                "{label}: Cochlea {feature} count lost events: candidate={candidate}, control={control}"
            ),
        ));
        return;
    }
    let tolerance = count_tolerance(control, 1);
    if candidate.saturating_sub(control) > tolerance {
        failures.push(SignalFailure::new(
            SignalFailureKind::RhythmicEventDivergence,
            format!(
                "{label}: Cochlea {feature} count diverged: candidate={candidate}, control={control}, tolerance={tolerance}"
            ),
        ));
    }
}

fn count_tolerance(reference: usize, minimum: usize) -> usize {
    reference.div_ceil(4).max(minimum)
}

fn post_sync_tempo_failures(
    captures: &[PcmCapture],
    reports: &BTreeMap<String, CochleaReport>,
    expected_bpm: f64,
    failures: &mut Vec<SignalFailure>,
) {
    for (deck, capture) in captures.iter().enumerate() {
        let Some(report) = reports.get(&capture.label) else {
            failures.push(SignalFailure::new(
                SignalFailureKind::PostSyncTempo,
                format!(
                    "post-SYNC: deck {deck} ({}) has no Cochlea report",
                    capture.label
                ),
            ));
            continue;
        };
        let Some(measured_bpm) = beat_grid_bpm(report).or(report.tempo_bpm) else {
            continue;
        };
        let expected_bin = cochlea_tempo_bin(capture.sample_rate, expected_bpm);
        if !measured_bpm.is_finite()
            || (measured_bpm - expected_bin).abs() > POST_SYNC_TEMPO_BIN_TOLERANCE_BPM
        {
            failures.push(SignalFailure::new(
                SignalFailureKind::PostSyncTempo,
                format!(
                    "post-SYNC: deck {deck} ({}) Cochlea tempo is {measured_bpm:.3} BPM, expected {expected_bpm:.3} BPM (representable bin {expected_bin:.3}) within +/-{POST_SYNC_TEMPO_BIN_TOLERANCE_BPM:.3} BPM",
                    capture.label
                ),
            ));
        }
    }
}

fn cochlea_tempo_bin(sample_rate: u32, bpm: f64) -> f64 {
    let frame_rate = f64::from(sample_rate) / COCHLEA_ONSET_HOP_FRAMES;
    let lag = (frame_rate * 60.0 / bpm).round().max(1.0);
    frame_rate * 60.0 / lag
}

fn beat_grid_bpm(report: &CochleaReport) -> Option<f64> {
    let mut intervals = report
        .beat_times_ms
        .windows(2)
        .filter_map(|beats| {
            let interval = beats[1] - beats[0];
            (interval.is_finite() && interval > 0.0).then_some(interval)
        })
        .collect::<Vec<_>>();
    if intervals.len() < 2 {
        return None;
    }
    intervals.sort_by(f64::total_cmp);
    let middle = intervals.len() / 2;
    let median = if intervals.len().is_multiple_of(2) {
        (intervals[middle - 1] + intervals[middle]) / 2.0
    } else {
        intervals[middle]
    };
    Some(60_000.0 / median)
}

fn phase_spread_frames(
    captures: &[PcmCapture],
    reports: &BTreeMap<String, CochleaReport>,
    session_bpm: f64,
    stage: &str,
    failure_kind: SignalFailureKind,
    failures: &mut Vec<SignalFailure>,
) -> Option<u64> {
    let beats = captures
        .iter()
        .map(|capture| {
            let report = reports.get(&capture.label)?;
            let frames = report
                .beat_times_ms
                .iter()
                .map(|milliseconds| {
                    capture.start_session_frame
                        + (milliseconds * f64::from(capture.sample_rate) / 1_000.0).round() as i64
                })
                .collect::<Vec<_>>();
            Some((capture, report, frames))
        })
        .collect::<Option<Vec<_>>>()?;
    if beats.len() < 2 {
        failures.push(SignalFailure::new(
            failure_kind,
            format!("{stage}: fewer than two deck captures were available"),
        ));
        return None;
    }
    for (deck, (capture, report, frames)) in beats.iter().enumerate() {
        if report.tempo_bpm.is_none() || frames.len() < MIN_MATCHED_BEATS {
            failures.push(SignalFailure::new(
                failure_kind,
                format!(
                    "{stage}: Cochlea did not estimate tempo with at least {MIN_MATCHED_BEATS} beats for deck {deck} ({}): tempo={:?}, confidence={:.6}, beats={}",
                    capture.label,
                    report.tempo_bpm,
                    report.tempo_confidence,
                    frames.len(),
                ),
            ));
            return None;
        }
        if !capture_phase_oracle_is_load_bearing(
            capture,
            report,
            stage,
            deck,
            failure_kind,
            failures,
        ) {
            return None;
        }
    }

    let sample_rate = captures[0].sample_rate;
    let beat_period = (f64::from(sample_rate) * 60.0 / session_bpm).round() as u64;
    let phases = beats
        .iter()
        .enumerate()
        .map(|(deck, (capture, _, deck_beats))| {
            let Some((phase, concentration)) = circular_phase(deck_beats, beat_period) else {
                failures.push(SignalFailure::new(
                    failure_kind,
                    format!(
                        "{stage}: deck {deck} ({}) has no representable Cochlea phase",
                        capture.label
                    ),
                ));
                return None;
            };
            if concentration < 0.5 {
                failures.push(SignalFailure::new(
                    failure_kind,
                    format!(
                        "{stage}: deck {deck} ({}) has unstable Cochlea phase concentration {concentration:.6}",
                        capture.label
                    ),
                ));
                return None;
            }
            Some(phase)
        })
        .collect::<Option<Vec<_>>>()?;
    circular_spread(&phases, beat_period)
}

fn circular_phase(frames: &[i64], period: u64) -> Option<(u64, f64)> {
    if frames.is_empty() || period == 0 || period > i64::MAX as u64 {
        return None;
    }
    let period_i64 = period as i64;
    let period_f64 = period as f64;
    let (sin_sum, cos_sum) = frames.iter().fold((0.0_f64, 0.0_f64), |(sin, cos), frame| {
        let phase = frame.rem_euclid(period_i64) as f64 / period_f64 * std::f64::consts::TAU;
        (sin + phase.sin(), cos + phase.cos())
    });
    let count = frames.len() as f64;
    let concentration = sin_sum.hypot(cos_sum) / count;
    let angle = sin_sum.atan2(cos_sum).rem_euclid(std::f64::consts::TAU);
    let phase = (angle / std::f64::consts::TAU * period_f64).round() as u64 % period;
    Some((phase, concentration))
}

fn circular_spread(phases: &[u64], period: u64) -> Option<u64> {
    if phases.len() < 2 || period == 0 {
        return None;
    }
    let mut phases = phases.to_vec();
    phases.sort_unstable();
    let largest_gap = phases
        .windows(2)
        .map(|window| window[1] - window[0])
        .chain(std::iter::once(
            period - phases[phases.len() - 1] + phases[0],
        ))
        .max()?;
    Some(period - largest_gap)
}

fn capture_phase_oracle_is_load_bearing(
    capture: &PcmCapture,
    report: &CochleaReport,
    stage: &str,
    deck: usize,
    failure_kind: SignalFailureKind,
    failures: &mut Vec<SignalFailure>,
) -> bool {
    let channels = usize::from(capture.channels);
    let shift_samples = COCHLEA_PHASE_CALIBRATION_SHIFT_FRAMES.saturating_mul(channels);
    if channels == 0 || capture.samples.len() <= shift_samples {
        failures.push(SignalFailure::new(
            failure_kind,
            format!(
                "{stage}: deck {deck} ({}) is too short for the {COCHLEA_PHASE_CALIBRATION_SHIFT_FRAMES}-frame Cochlea phase calibration",
                capture.label
            ),
        ));
        return false;
    }

    let mut shifted = vec![0.0_f32; shift_samples];
    shifted.extend_from_slice(&capture.samples[..capture.samples.len() - shift_samples]);
    let shifted_report = CochleaReport::measure(&shifted, capture.channels, capture.sample_rate);
    let Some(original_bpm) = report.tempo_bpm else {
        return false;
    };
    let Some(shifted_bpm) = shifted_report.tempo_bpm else {
        failures.push(SignalFailure::new(
            failure_kind,
            format!(
                "{stage}: shifted calibration lost the tempo estimate for deck {deck} ({}); original clear_rhythm={}, confidence={:.6}",
                capture.label, report.clear_rhythm, report.tempo_confidence
            ),
        ));
        return false;
    };
    let tempo_tolerance = (original_bpm * 0.05).max(3.0);
    let offsets = report
        .beat_times_ms
        .iter()
        .zip(&shifted_report.beat_times_ms)
        .take(8)
        .map(|(original, shifted)| (shifted - original).abs())
        .collect::<Vec<_>>();
    let expected_shift_ms =
        COCHLEA_PHASE_CALIBRATION_SHIFT_FRAMES as f64 / f64::from(capture.sample_rate) * 1_000.0;
    let low_confidence = !report.clear_rhythm
        || !shifted_report.clear_rhythm
        || report.tempo_confidence < 0.01
        || shifted_report.tempo_confidence < 0.01;
    let required_displacements = if low_confidence { 3 } else { 2 };
    let displaced = offsets
        .iter()
        .filter(|offset| **offset >= expected_shift_ms / 2.0)
        .count();
    let load_bearing = (shifted_bpm - original_bpm).abs() <= tempo_tolerance
        && offsets.len() >= MIN_MATCHED_BEATS
        && displaced >= required_displacements;
    if !load_bearing {
        failures.push(SignalFailure::new(
            failure_kind,
            format!(
                "{stage}: Cochlea phase estimate is not load-bearing for deck {deck} ({}): clear_rhythm={}, confidence={:.6}, shifted_clear_rhythm={}, shifted_confidence={:.6}, bpm={original_bpm:.3}/{shifted_bpm:.3}, injected_shift_ms={expected_shift_ms:.3}, offsets_ms={offsets:?}",
                capture.label,
                report.clear_rhythm,
                report.tempo_confidence,
                shifted_report.clear_rhythm,
                shifted_report.tempo_confidence,
            ),
        ));
    }
    load_bearing
}

#[cfg(test)]
mod tests {
    use ::kithara::audio::{BeatMapId, BeatMapRevision, BeatOrdinal, MapStamp, Meter};
    use kithara_test_utils::kithara;

    use super::*;
    use crate::cochlea::rhythmic_calibration;

    const CHANNELS: usize = 2;
    const SAMPLE_RATE: u32 = 48_000;
    const BPM: usize = 120;
    const EVENT_INDEX: usize = 12;
    const COCHLEA_INJECTION_FRAMES: usize = 512;

    #[kithara::test]
    fn continuity_comparator_rejects_one_render_quantum_dropout() {
        let control = rhythmic_calibration(CHANNELS, SAMPLE_RATE);
        let mut candidate = control.clone();
        let event_start = EVENT_INDEX * beat_frames();
        let gap_start = event_start + usize::try_from(SAMPLE_RATE).unwrap_or(usize::MAX) / 50;
        silence_frames(&mut candidate, gap_start, COCHLEA_INJECTION_FRAMES);

        let control_report = measure(&control);
        let candidate_report = measure(&candidate);
        assert!(
            compare_continuity("identical calibration", &control_report, &control_report)
                .is_empty(),
            "the comparator must accept an identical Cochlea report"
        );
        assert!(
            candidate_report.silent_segments > control_report.silent_segments,
            "the 512-frame calibration dropout must create an extra Cochlea silent segment: candidate={}, control={}",
            candidate_report.silent_segments,
            control_report.silent_segments,
        );

        let failures = compare_continuity(
            "512-frame calibration dropout",
            &candidate_report,
            &control_report,
        );
        assert!(
            failures
                .iter()
                .any(|failure| failure.message().contains("extra silent segments")),
            "the actual matrix comparator accepted a 512-frame dropout: {failures:?}"
        );
    }

    #[kithara::test]
    fn exact_stamp_oracle_rejects_one_frame_activation_shift() {
        let control = phase_observation();
        assert!(
            exact_phase_failures(std::slice::from_ref(&control)).is_empty(),
            "an exact admitted-map observation must pass"
        );

        let mut shifted = control;
        shifted.applied_activation_frame += 1;
        let failures = exact_phase_failures(std::slice::from_ref(&shifted));

        assert!(
            failures.iter().any(|failure| failure
                .message()
                .contains("activation stamp shifted by 1 frame(s)")),
            "the exact stamp oracle accepted a one-frame activation shift: {failures:?}"
        );
    }

    #[kithara::test]
    fn exact_phase_oracle_rejects_one_beat_bar_phase_error() {
        let control = phase_observation();
        assert!(
            exact_phase_failures(std::slice::from_ref(&control)).is_empty(),
            "an exact admitted-map observation must pass"
        );

        let mut shifted = control;
        shifted.observed_beat = BeatOrdinal::new(i64::from(control.expected_beat) + 1);
        let failures = exact_phase_failures(std::slice::from_ref(&shifted));

        assert!(
            failures
                .iter()
                .any(|failure| failure.message().contains("resolved beat ordinal")),
            "the exact phase oracle accepted a one-beat ordinal error: {failures:?}"
        );
        assert!(
            failures
                .iter()
                .any(|failure| failure.message().contains("bar phase")),
            "the exact phase oracle accepted a one-beat bar-phase error: {failures:?}"
        );
    }

    #[kithara::test]
    fn continuity_comparator_rejects_one_missing_rhythmic_event() {
        let control = rhythmic_calibration(CHANNELS, SAMPLE_RATE);
        let mut candidate = control.clone();
        let event_start = EVENT_INDEX * beat_frames();
        silence_frames(&mut candidate, event_start, beat_frames() / 10);

        let control_report = measure(&control);
        let candidate_report = measure(&candidate);
        assert!(
            candidate_report.onset_count() < control_report.onset_count(),
            "the missing-event calibration must remove a Cochlea onset: candidate={}, control={}",
            candidate_report.onset_count(),
            control_report.onset_count(),
        );

        let failures =
            compare_continuity("missing rhythmic event", &candidate_report, &control_report);
        assert!(
            failures
                .iter()
                .any(|failure| failure.message().contains("onsets count lost events")),
            "the actual matrix comparator accepted a missing rhythmic event: {failures:?}"
        );
    }

    #[kithara::test]
    fn post_sync_tempo_oracle_rejects_120_bpm_for_127_bpm_target() {
        let samples = rhythmic_calibration(CHANNELS, SAMPLE_RATE);
        let report = measure(&samples);
        let measured_bpm = report
            .tempo_bpm
            .expect("rhythmic calibration must have a Cochlea tempo");
        let expected_bin = cochlea_tempo_bin(SAMPLE_RATE, BPM as f64);
        assert!(
            (measured_bpm - expected_bin).abs() <= POST_SYNC_TEMPO_BIN_TOLERANCE_BPM,
            "calibration must provide the representable 120 BPM input bin {expected_bin:.3}: {measured_bpm:.3}"
        );

        let capture = PcmCapture {
            channels: CHANNELS as u16,
            label: "post-sync-120-bpm".to_owned(),
            sample_rate: SAMPLE_RATE,
            samples,
            start_session_frame: 0,
        };
        let reports = BTreeMap::from([(capture.label.clone(), report)]);

        let mut matched_failures = Vec::new();
        post_sync_tempo_failures(
            std::slice::from_ref(&capture),
            &reports,
            BPM as f64,
            &mut matched_failures,
        );
        assert!(
            matched_failures.is_empty(),
            "the tempo oracle rejected its matching 120 BPM target: {matched_failures:?}"
        );

        let mut mismatched_failures = Vec::new();
        post_sync_tempo_failures(
            std::slice::from_ref(&capture),
            &reports,
            127.0,
            &mut mismatched_failures,
        );
        assert!(
            mismatched_failures.iter().any(|failure| {
                failure.message().contains("Cochlea tempo is")
                    && failure.message().contains("expected 127.000 BPM")
            }),
            "the tempo oracle accepted 120 BPM for a 127 BPM target: {mismatched_failures:?}"
        );
    }

    fn beat_frames() -> usize {
        usize::try_from(SAMPLE_RATE).unwrap_or(usize::MAX) * 60 / BPM
    }

    fn phase_observation() -> LockedPhaseObservation {
        let map = MapStamp::new(
            BeatMapId::allocate().expect("invariant: fixture map identity is available"),
            BeatMapRevision::first(),
        );
        LockedPhaseObservation {
            admitted_map: map,
            applied_activation_frame: 96_000,
            applied_map: map,
            deck: 1,
            expected_activation_frame: 96_000,
            expected_beat: BeatOrdinal::new(8),
            expected_phase_frame: 96_000,
            meter: Meter::new(4).expect("invariant: fixture meter is valid"),
            observed_beat: BeatOrdinal::new(8),
            observed_phase_frame: 96_001,
        }
    }

    fn measure(samples: &[f32]) -> CochleaReport {
        CochleaReport::measure(samples, CHANNELS as u16, SAMPLE_RATE)
    }

    fn silence_frames(samples: &mut [f32], start_frame: usize, frames: usize) {
        let start = start_frame.saturating_mul(CHANNELS);
        let end = start
            .saturating_add(frames.saturating_mul(CHANNELS))
            .min(samples.len());
        assert_eq!(
            end.saturating_sub(start),
            frames.saturating_mul(CHANNELS),
            "calibration injection must fit in the signal"
        );
        samples[start..end].fill(0.0);
    }
}
