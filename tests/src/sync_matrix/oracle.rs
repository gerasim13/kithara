use std::collections::BTreeMap;

use anyhow::{Result, bail};

use super::{CaptureBundle, PcmCapture, SYNC_FRAME_BUDGET, SyncCase};
use crate::{
    cochlea::{CochleaReport, DEFAULT_WINDOW_MS, assert_rhythmic_oracle_load_bearing},
    sync_artifact::{
        ArtifactAudio, ArtifactFrame, ArtifactSource, SyncArtifactMetadata, write_sync_artifact,
    },
};

const MIN_MATCHED_BEATS: usize = 3;
// Six-beat captures can shift Cochlea's finite-window estimate slightly. Half a BPM
// absorbs that quantization without accepting a musically distinct whole-BPM drift.
const POST_SYNC_TEMPO_TOLERANCE_BPM: f64 = 0.5;

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct SyncOracleReport {
    failures: Vec<String>,
    post_sync_spread_frames: Option<u64>,
    pre_sync_spread_frames: Option<u64>,
}

pub struct SyncOracle;

impl SyncOracle {
    #[must_use]
    pub fn evaluate(case: SyncCase, bundle: &CaptureBundle) -> SyncOracleReport {
        let cochlea = bundle
            .audio()
            .map(|capture| {
                (
                    capture.label.clone(),
                    CochleaReport::measure(&capture.samples, capture.channels, capture.sample_rate),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mut failures = capture_failures(case, bundle);
        failures.extend(
            bundle
                .capture_failures
                .iter()
                .map(|failure| format!("capture: {failure}")),
        );
        failures.extend(continuity_failures_for_bundle(bundle, &cochlea));

        let pre_sync_spread_frames = phase_spread_frames(
            &bundle.pre_sync_replays,
            &cochlea,
            case.session_bpm,
            "pre-SYNC",
            &mut failures,
        );
        match pre_sync_spread_frames {
            Some(spread) if spread > SYNC_FRAME_BUDGET => {}
            Some(spread) => failures.push(format!(
                "pre-SYNC Cochlea beat spread is {spread} frames; stagger was not proven beyond the {SYNC_FRAME_BUDGET}-frame sync budget"
            )),
            None => {}
        }

        let final_bpm = case.tempo_ride.final_bpm();
        post_sync_tempo_failures(&bundle.deck_replays, &cochlea, final_bpm, &mut failures);
        let post_sync_spread_frames = phase_spread_frames(
            &bundle.deck_replays,
            &cochlea,
            final_bpm,
            "post-SYNC",
            &mut failures,
        );
        if let Some(spread) = post_sync_spread_frames
            && spread > SYNC_FRAME_BUDGET
        {
            failures.push(format!(
                "post-SYNC Cochlea beat spread is {spread} frames; budget is {SYNC_FRAME_BUDGET} frames"
            ));
        }

        SyncOracleReport {
            failures,
            post_sync_spread_frames,
            pre_sync_spread_frames,
        }
    }
}

fn capture_failures(case: SyncCase, bundle: &CaptureBundle) -> Vec<String> {
    let mut failures = Vec::new();
    if bundle.facts.abr_switch_failures > 0
        || bundle.facts.abr_switches != bundle.facts.abr_switches_expected
    {
        failures.push(format!(
            "HLS ABR switch coverage failed: applied={}, expected={}, failures={}",
            bundle.facts.abr_switches,
            bundle.facts.abr_switches_expected,
            bundle.facts.abr_switch_failures,
        ));
    }
    if bundle.facts.underruns > 0 {
        failures.push(format!(
            "candidate lifecycle emitted {} AudioEvent::UnderrunStarted events",
            bundle.facts.underruns
        ));
    }
    if bundle.facts.event_lagged > 0 || bundle.facts.event_streams_closed > 0 {
        failures.push(format!(
            "candidate event observation was incomplete: lagged={}, closed_streams={}",
            bundle.facts.event_lagged, bundle.facts.event_streams_closed,
        ));
    }
    let expected_bpm = case.tempo_ride.final_bpm();
    if (bundle.facts.final_session_bpm - expected_bpm).abs() > 1.0e-6 {
        failures.push(format!(
            "final session tempo is {:.6} BPM, expected {:.6} BPM",
            bundle.facts.final_session_bpm, expected_bpm
        ));
    }
    for capture in bundle.audio() {
        if !capture.backend_matches_tap {
            failures.push(format!(
                "{}: final session mix tap differs from rendered backend PCM",
                capture.label
            ));
        }
        if capture.tap_dropped_samples != 0 {
            failures.push(format!(
                "{}: final session mix tap dropped {} samples",
                capture.label, capture.tap_dropped_samples
            ));
        }
        if capture.samples.is_empty() {
            failures.push(format!("{}: final rendered PCM is empty", capture.label));
        }
    }
    deck_outcome_failures(case, bundle, &mut failures);
    if bundle.facts.rebinds != 1 {
        failures.push(format!(
            "SYNC re-enable lifecycle count is {}, expected 1",
            bundle.facts.rebinds
        ));
    }
    if bundle.facts.reloads != case.decks {
        failures.push(format!(
            "track reload lifecycle count is {}, expected {}",
            bundle.facts.reloads, case.decks
        ));
    }
    let expected_republishes = case.decks.saturating_mul(3);
    if bundle.facts.map_withdrawals != case.decks
        || bundle.facts.map_republishes != expected_republishes
    {
        failures.push(format!(
            "BeatMap readiness/refinement lifecycle mismatch: withdrawals={}/{}, republishes={}/{}",
            bundle.facts.map_withdrawals,
            case.decks,
            bundle.facts.map_republishes,
            expected_republishes,
        ));
    }
    if bundle.facts.map_unavailable_errors != bundle.facts.map_withdrawals {
        failures.push(format!(
            "BeatMap withdrawal did not fail closed: unavailable_errors={}/{}",
            bundle.facts.map_unavailable_errors, bundle.facts.map_withdrawals,
        ));
    }
    let expected_ride_points = case.tempo_ride.update_count(case.tempo_updates_hz);
    if bundle.facts.tempo_ride_requests != expected_ride_points {
        failures.push(format!(
            "session tempo ride issued {} requests, expected {}",
            bundle.facts.tempo_ride_requests, expected_ride_points
        ));
    }
    if bundle
        .facts
        .tempo_ride_points
        .saturating_add(bundle.facts.tempo_ride_transport_not_processed)
        != bundle.facts.tempo_ride_requests
    {
        failures.push(format!(
            "session tempo ride accounting mismatch: requests={}, applied={}, transport_not_processed={}",
            bundle.facts.tempo_ride_requests,
            bundle.facts.tempo_ride_points,
            bundle.facts.tempo_ride_transport_not_processed,
        ));
    }
    if bundle.facts.tempo_ride_transport_not_processed > 0 {
        failures.push(format!(
            "session tempo ride rejected {} requests with TransportNotProcessed",
            bundle.facts.tempo_ride_transport_not_processed
        ));
    }
    if bundle.facts.tempo_ride_points != expected_ride_points {
        failures.push(format!(
            "session tempo ride applied {} points, expected {}",
            bundle.facts.tempo_ride_points, expected_ride_points
        ));
    }
    tempo_request_cadence_failure(case, &bundle.ledger, &mut failures);
    failures
}

fn deck_outcome_failures(case: SyncCase, bundle: &CaptureBundle, failures: &mut Vec<String>) {
    for (deck, outcome) in bundle.facts.deck_outcomes.iter().enumerate() {
        if outcome.track_failed {
            failures.push(format!(
                "deck {deck}: track failed during the behavioral row"
            ));
        }
        if !outcome.playing {
            failures.push(format!(
                "deck {deck}: playback stopped during the behavioral row"
            ));
        }
        if outcome.current_index != outcome.expected_index {
            failures.push(format!(
                "deck {deck}: queue index changed during SYNC: current={:?}, expected={:?}",
                outcome.current_index, outcome.expected_index
            ));
        }
        if outcome.expected_index.is_none() {
            failures.push(format!("deck {deck}: final queue index was never bound"));
        }
        if !outcome.rate.is_finite() || (outcome.rate - outcome.expected_rate).abs() > 1.0e-4 {
            failures.push(format!(
                "deck {deck}: final live rate is {:.6}, expected {:.6}",
                outcome.rate, outcome.expected_rate
            ));
        }
    }
    if bundle.facts.deck_outcomes.len() != case.decks {
        failures.push(format!(
            "candidate reported {} final deck outcomes, expected {}",
            bundle.facts.deck_outcomes.len(),
            case.decks
        ));
    }
}

fn tempo_request_cadence_failure(
    case: SyncCase,
    ledger: &[super::LedgerEntry],
    failures: &mut Vec<String>,
) {
    let requests = ledger
        .iter()
        .filter(|entry| entry.event == "tempo-ride-request")
        .collect::<Vec<_>>();
    let Some(first) = requests.first() else {
        failures.push("session tempo ride emitted no request timestamps".to_owned());
        return;
    };
    let rate = u64::from(case.sample_rate);
    let frequency = u64::from(case.tempo_updates_hz);
    for (ordinal, entry) in requests.iter().enumerate() {
        let expected = first.frame.saturating_add(
            u64::try_from(ordinal)
                .unwrap_or(u64::MAX)
                .saturating_mul(rate)
                / frequency,
        );
        if entry.frame != expected {
            failures.push(format!(
                "tempo request {ordinal} landed at frame {}, expected {expected} for {} Hz",
                entry.frame, case.tempo_updates_hz
            ));
            break;
        }
    }
}

fn continuity_failures_for_bundle(
    bundle: &CaptureBundle,
    reports: &BTreeMap<String, CochleaReport>,
) -> Vec<String> {
    let mut failures = Vec::new();
    if let (Some(candidate), Some(control)) = (
        reports.get(&bundle.mix.label),
        reports.get(&bundle.control_mix.label),
    ) {
        failures.extend(compare_continuity(
            "shared lifecycle mix",
            candidate,
            control,
        ));
    }
    for (deck, (candidate, control)) in bundle
        .deck_replays
        .iter()
        .zip(&bundle.control_replays)
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
) -> Vec<String> {
    let mut failures = Vec::new();
    if candidate.silent_segments > control.silent_segments {
        failures.push(format!(
            "{label}: Cochlea found extra silent segments: candidate={}, control={}",
            candidate.silent_segments, control.silent_segments,
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
        failures.push(format!(
            "{label}: Cochlea found {} clipped candidate samples; control clipping cannot excuse candidate clipping",
            candidate.clipped_samples
        ));
    }
    if candidate.true_peak_over_0dbtp {
        failures.push(format!(
            "{label}: Cochlea found a candidate true peak over 0 dBTP"
        ));
    }
    if control.clipped_samples > 0 || control.true_peak_over_0dbtp {
        failures.push(format!(
            "{label}: conservative free control is not clean: clipped_samples={}, true_peak_over_0dbtp={}",
            control.clipped_samples, control.true_peak_over_0dbtp
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

fn compare_rhythmic_count(
    label: &str,
    feature: &str,
    candidate: usize,
    control: usize,
    failures: &mut Vec<String>,
) {
    if candidate < control {
        failures.push(format!(
            "{label}: Cochlea {feature} count lost events: candidate={candidate}, control={control}"
        ));
        return;
    }
    let tolerance = count_tolerance(control, 1);
    if candidate.saturating_sub(control) > tolerance {
        failures.push(format!(
            "{label}: Cochlea {feature} count diverged: candidate={candidate}, control={control}, tolerance={tolerance}"
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
    failures: &mut Vec<String>,
) {
    for (deck, capture) in captures.iter().enumerate() {
        let Some(report) = reports.get(&capture.label) else {
            failures.push(format!(
                "post-SYNC: deck {deck} ({}) has no Cochlea report",
                capture.label
            ));
            continue;
        };
        let Some(measured_bpm) = report.tempo_bpm else {
            continue;
        };
        if !measured_bpm.is_finite()
            || (measured_bpm - expected_bpm).abs() > POST_SYNC_TEMPO_TOLERANCE_BPM
        {
            failures.push(format!(
                "post-SYNC: deck {deck} ({}) Cochlea tempo is {measured_bpm:.3} BPM, expected {expected_bpm:.3} BPM within +/-{POST_SYNC_TEMPO_TOLERANCE_BPM:.3} BPM",
                capture.label
            ));
        }
    }
}

fn phase_spread_frames(
    captures: &[PcmCapture],
    reports: &BTreeMap<String, CochleaReport>,
    session_bpm: f64,
    stage: &str,
    failures: &mut Vec<String>,
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
        failures.push(format!(
            "{stage}: fewer than two deck captures were available"
        ));
        return None;
    }
    for (deck, (capture, report, frames)) in beats.iter().enumerate() {
        if report.tempo_bpm.is_none() || frames.len() < MIN_MATCHED_BEATS {
            failures.push(format!(
                "{stage}: Cochlea did not estimate tempo with at least {MIN_MATCHED_BEATS} beats for deck {deck} ({}): tempo={:?}, confidence={:.6}, beats={}",
                capture.label,
                report.tempo_bpm,
                report.tempo_confidence,
                frames.len(),
            ));
            return None;
        }
        if !capture_phase_oracle_is_load_bearing(capture, report, stage, deck, failures) {
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
                failures.push(format!(
                    "{stage}: deck {deck} ({}) has no representable Cochlea phase",
                    capture.label
                ));
                return None;
            };
            if concentration < 0.5 {
                failures.push(format!(
                    "{stage}: deck {deck} ({}) has unstable Cochlea phase concentration {concentration:.6}",
                    capture.label
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
    failures: &mut Vec<String>,
) -> bool {
    let channels = usize::from(capture.channels);
    let shift_samples = (SYNC_FRAME_BUDGET as usize).saturating_mul(channels);
    if channels == 0 || capture.samples.len() <= shift_samples {
        failures.push(format!(
            "{stage}: deck {deck} ({}) is too short for the {SYNC_FRAME_BUDGET}-frame Cochlea phase calibration",
            capture.label
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
        failures.push(format!(
            "{stage}: shifted calibration lost the tempo estimate for deck {deck} ({}); original clear_rhythm={}, confidence={:.6}",
            capture.label, report.clear_rhythm, report.tempo_confidence
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
    let expected_shift_ms = SYNC_FRAME_BUDGET as f64 / f64::from(capture.sample_rate) * 1_000.0;
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
        failures.push(format!(
            "{stage}: Cochlea phase estimate is not load-bearing for deck {deck} ({}): clear_rhythm={}, confidence={:.6}, shifted_clear_rhythm={}, shifted_confidence={:.6}, bpm={original_bpm:.3}/{shifted_bpm:.3}, injected_shift_ms={expected_shift_ms:.3}, offsets_ms={offsets:?}",
            capture.label,
            report.clear_rhythm,
            report.tempo_confidence,
            shifted_report.clear_rhythm,
            shifted_report.tempo_confidence,
        ));
    }
    load_bearing
}

pub fn persist_then_assert(
    case: SyncCase,
    bundle: &CaptureBundle,
    report: &SyncOracleReport,
) -> Result<()> {
    let mut metadata = SyncArtifactMetadata::new(
        format!("{}-{}", case.id, bundle.media_id),
        case.sample_rate,
        super::CHANNELS,
        super::RENDER_FRAMES,
    );
    metadata.set_operation(case.order.label());
    if let Some(seed) = bundle.library_seed {
        metadata.set_library_seed(seed);
    }
    for source in &bundle.sources {
        metadata.add_source(
            ArtifactSource::new(&source.deck, &source.media)
                .with_analysis_key(&source.analysis_key),
        );
    }
    for entry in &bundle.ledger {
        metadata.add_frame(ArtifactFrame::new(entry.frame, entry.event));
    }
    metadata.add_threshold("sync_frame_budget", SYNC_FRAME_BUDGET as f64);
    if let Some(spread) = report.pre_sync_spread_frames {
        metadata.add_state("pre_sync_spread_frames", spread.to_string());
    }
    if let Some(spread) = report.post_sync_spread_frames {
        metadata.add_state("post_sync_spread_frames", spread.to_string());
    }
    metadata.add_state(
        "final_session_bpm",
        bundle.facts.final_session_bpm.to_string(),
    );
    metadata.add_state("candidate_underruns", bundle.facts.underruns.to_string());
    metadata.add_state("event_lagged", bundle.facts.event_lagged.to_string());
    metadata.add_state(
        "event_streams_closed",
        bundle.facts.event_streams_closed.to_string(),
    );
    metadata.add_state(
        "map_unavailable_errors",
        bundle.facts.map_unavailable_errors.to_string(),
    );
    metadata.add_state(
        "tempo_ride_requests",
        bundle.facts.tempo_ride_requests.to_string(),
    );
    metadata.add_state(
        "tempo_ride_applied",
        bundle.facts.tempo_ride_points.to_string(),
    );
    metadata.add_state(
        "tempo_ride_rejected_transport_not_processed",
        bundle.facts.tempo_ride_transport_not_processed.to_string(),
    );
    for (deck, outcome) in bundle.facts.deck_outcomes.iter().enumerate() {
        metadata.add_state(format!("deck_{deck}_rate"), outcome.rate.to_string());
        metadata.add_state(
            format!("deck_{deck}_current_index"),
            format!("{:?}", outcome.current_index),
        );
    }
    metadata.add_failures(report.failures.clone());
    let audio = bundle
        .audio()
        .map(|capture| ArtifactAudio::new(&capture.label, &capture.samples))
        .collect::<Vec<_>>();
    let _ = write_sync_artifact(&metadata, &audio)?;

    assert_rhythmic_oracle_load_bearing();
    if !report.failures.is_empty() {
        bail!(
            "{}: sync PCM acceptance failed:\n{}",
            case,
            report.failures.join("\n")
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cochlea::rhythmic_calibration;

    const CHANNELS: usize = 2;
    const SAMPLE_RATE: u32 = 48_000;
    const BPM: usize = 120;
    const EVENT_INDEX: usize = 12;
    const QUANTUM_FRAMES: usize = 512;

    #[test]
    fn continuity_comparator_rejects_one_render_quantum_dropout() {
        let control = rhythmic_calibration(CHANNELS, SAMPLE_RATE);
        let mut candidate = control.clone();
        let event_start = EVENT_INDEX * beat_frames();
        let gap_start = event_start + usize::try_from(SAMPLE_RATE).unwrap_or(usize::MAX) / 50;
        silence_frames(&mut candidate, gap_start, QUANTUM_FRAMES);

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
                .any(|failure| failure.contains("extra silent segments")),
            "the actual matrix comparator accepted a 512-frame dropout: {failures:?}"
        );
    }

    #[test]
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
                .any(|failure| failure.contains("onsets count lost events")),
            "the actual matrix comparator accepted a missing rhythmic event: {failures:?}"
        );
    }

    #[test]
    fn post_sync_tempo_oracle_rejects_120_bpm_for_127_bpm_target() {
        let samples = rhythmic_calibration(CHANNELS, SAMPLE_RATE);
        let report = measure(&samples);
        let measured_bpm = report
            .tempo_bpm
            .expect("rhythmic calibration must have a Cochlea tempo");
        assert!(
            (measured_bpm - BPM as f64).abs() <= POST_SYNC_TEMPO_TOLERANCE_BPM,
            "calibration must provide the independent 120 BPM input: {measured_bpm:.3}"
        );

        let capture = PcmCapture {
            backend_matches_tap: true,
            channels: CHANNELS as u16,
            label: "post-sync-120-bpm".to_owned(),
            sample_rate: SAMPLE_RATE,
            samples,
            start_session_frame: 0,
            tap_dropped_samples: 0,
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
                failure.contains("Cochlea tempo is") && failure.contains("expected 127.000 BPM")
            }),
            "the tempo oracle accepted 120 BPM for a 127 BPM target: {mismatched_failures:?}"
        );
    }

    fn beat_frames() -> usize {
        usize::try_from(SAMPLE_RATE).unwrap_or(usize::MAX) * 60 / BPM
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
