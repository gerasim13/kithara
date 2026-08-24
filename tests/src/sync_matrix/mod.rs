mod capture;
mod case;
mod harness;
mod media;
mod oracle;
mod replay;

pub use capture::CaptureBundle;
use capture::{
    CaptureSource, DeckOutcome, LedgerEntry, LockedPhaseObservation, PcmCapture, ScenarioFacts,
};
use case::{
    CHANNELS, COCHLEA_PHASE_SPREAD_BUDGET_FRAMES, InitialDeckState, MAX_LOCKED_PHASE_ERROR_FRAMES,
    Operation, RENDER_FRAMES,
};
pub use case::{OperationOrder, SyncCase, TempoRide};
use harness::SyncHarness;
pub use media::{SyncMedia, SyncTrackFixture};
pub use oracle::{SyncOracle, SyncOracleReport, persist_then_assert};
pub use replay::{assert_behavioral_row, run_synthetic_behavioral_row};
