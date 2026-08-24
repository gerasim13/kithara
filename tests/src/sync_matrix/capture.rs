#[derive(Clone, Debug)]
pub(super) struct PcmCapture {
    pub(super) channels: u16,
    pub(super) label: String,
    pub(super) sample_rate: u32,
    pub(super) samples: Vec<f32>,
    pub(super) start_session_frame: i64,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct LedgerEntry {
    pub(super) event: &'static str,
    pub(super) frame: u64,
}

#[derive(Clone, Debug)]
pub(super) struct CaptureSource {
    pub(super) analysis_key: String,
    pub(super) deck: String,
    pub(super) media: String,
}

#[derive(Clone, Debug)]
pub(super) struct DeckOutcome {
    pub(super) current_index: Option<usize>,
    pub(super) expected_index: Option<usize>,
    pub(super) expected_rate: f32,
    pub(super) playing: bool,
    pub(super) rate: f32,
    pub(super) track_failed: bool,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct LockedPhaseObservation {
    pub(super) admitted_map: MapStamp,
    pub(super) applied_activation_frame: i64,
    pub(super) applied_map: MapStamp,
    pub(super) deck: usize,
    pub(super) expected_activation_frame: i64,
    pub(super) expected_beat: BeatOrdinal,
    pub(super) expected_phase_frame: i64,
    pub(super) meter: Meter,
    pub(super) observed_beat: BeatOrdinal,
    pub(super) observed_phase_frame: i64,
}

#[derive(Clone, Debug, Default)]
pub(super) struct ScenarioFacts {
    pub(super) abr_switch_failures: usize,
    pub(super) abr_switches: usize,
    pub(super) abr_switches_expected: usize,
    pub(super) deck_outcomes: Vec<DeckOutcome>,
    pub(super) event_lagged: u64,
    pub(super) event_streams_closed: usize,
    pub(super) final_session_bpm: f64,
    pub(super) map_unavailable_errors: usize,
    pub(super) map_withdrawals: usize,
    pub(super) map_republishes: usize,
    pub(super) reloads: usize,
    pub(super) rebinds: usize,
    pub(super) tempo_ride_points: usize,
    pub(super) tempo_ride_requests: usize,
    pub(super) tempo_ride_transport_not_processed: usize,
    pub(super) underruns: usize,
}

/// Audio evidence consumed by the shared sync oracle.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct SignalEvidence {
    pub(super) control_mix: PcmCapture,
    pub(super) control_replays: Vec<PcmCapture>,
    pub(super) deck_replays: Vec<PcmCapture>,
    pub(super) mix: PcmCapture,
    pub(super) phase_observations: Vec<LockedPhaseObservation>,
    pub(super) pre_sync_replays: Vec<PcmCapture>,
}

impl SignalEvidence {
    pub(super) fn audio(&self) -> impl Iterator<Item = &PcmCapture> {
        std::iter::once(&self.mix)
            .chain(std::iter::once(&self.control_mix))
            .chain(self.deck_replays.iter())
            .chain(self.control_replays.iter())
            .chain(self.pre_sync_replays.iter())
    }
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct CaptureBundle {
    pub(super) capture_failures: Vec<String>,
    pub(super) facts: ScenarioFacts,
    pub(super) ledger: Vec<LedgerEntry>,
    pub(super) library_seed: Option<u64>,
    pub(super) media_id: String,
    pub(super) signal: SignalEvidence,
    pub(super) sources: Vec<CaptureSource>,
}

impl CaptureBundle {
    pub(super) fn audio(&self) -> impl Iterator<Item = &PcmCapture> {
        self.signal.audio()
    }
}
use kithara::audio::{BeatOrdinal, MapStamp, Meter};
