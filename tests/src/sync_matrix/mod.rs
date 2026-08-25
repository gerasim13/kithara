mod capture;
mod case;
mod harness;
mod listening;
mod media;
mod oracle;
mod provider;
mod replay;
mod signal_oracle;

pub use capture::{CaptureBundle, SignalEvidence};
use capture::{
    CaptureSource, DeckOutcome, LedgerEntry, LockedPhaseObservation, PcmCapture, ScenarioFacts,
};
use case::{
    CHANNELS, COCHLEA_PHASE_SPREAD_BUDGET_FRAMES, InitialDeckState, MAX_LOCKED_PHASE_ERROR_FRAMES,
    Operation, RENDER_FRAMES,
};
pub use case::{OperationOrder, SyncCase, TempoRide};
use harness::SyncHarness;
pub use listening::write_sync_listening_dump;
pub use media::{SyncMedia, SyncTrackFixture};
pub use oracle::{SyncOracle, SyncOracleReport, persist_then_assert};
pub use provider::{
    AssetProvider, PlayerQueueProvider, SignalCapture, SignalDefect, SignalProvider,
    evaluate_signal,
};
pub use replay::{assert_behavioral_row, run_synthetic_behavioral_row};
pub use signal_oracle::{SignalFailure, SignalFailureKind, SignalOracle, SignalOracleReport};

pub use crate::signal_pcm::signal::RhythmicTrack;
