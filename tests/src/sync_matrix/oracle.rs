use anyhow::{Result, bail};

use super::{
    COCHLEA_PHASE_SPREAD_BUDGET_FRAMES, CaptureBundle, MAX_LOCKED_PHASE_ERROR_FRAMES,
    RENDER_FRAMES, SignalOracle, SyncCase, signal_oracle::COCHLEA_PHASE_CALIBRATION_SHIFT_FRAMES,
};
use crate::{
    cochlea::assert_rhythmic_oracle_load_bearing,
    sync_artifact::{
        ArtifactAudio, ArtifactFrame, ArtifactSource, SyncArtifactMetadata, write_sync_artifact,
    },
};

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
        let signal = SignalOracle::evaluate(case, &bundle.signal);
        let (signal_failures, post_sync_spread_frames, pre_sync_spread_frames) =
            signal.into_runtime_parts();
        let mut failures = capture_failures(case, bundle);
        failures.extend(
            bundle
                .capture_failures
                .iter()
                .map(|failure| format!("capture: {failure}")),
        );
        failures.extend(signal_failures);

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

pub fn persist_then_assert(
    case: SyncCase,
    bundle: &CaptureBundle,
    report: &SyncOracleReport,
) -> Result<()> {
    let mut metadata = SyncArtifactMetadata::new(
        format!("{}-{}", case.id, bundle.media_id),
        case.sample_rate,
        super::CHANNELS,
        RENDER_FRAMES,
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
    metadata.add_threshold("render_quantum_frames", RENDER_FRAMES as f64);
    metadata.add_threshold(
        "cochlea_phase_spread_budget_frames",
        COCHLEA_PHASE_SPREAD_BUDGET_FRAMES as f64,
    );
    metadata.add_threshold(
        "cochlea_phase_calibration_shift_frames",
        COCHLEA_PHASE_CALIBRATION_SHIFT_FRAMES as f64,
    );
    metadata.add_threshold(
        "max_locked_phase_error_frames",
        MAX_LOCKED_PHASE_ERROR_FRAMES as f64,
    );
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
