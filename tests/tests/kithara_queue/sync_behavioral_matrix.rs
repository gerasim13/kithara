#![cfg(not(target_arch = "wasm32"))]

use std::collections::BTreeSet;

use anyhow::Result;
use kithara::{
    bufpool::{BytePool, PcmPool},
    platform::time::Duration,
};
use kithara_integration_tests::{
    kithara,
    sync_fixture::SyncFixtureResources,
    sync_matrix::{
        AssetProvider, OperationOrder, PlayerQueueProvider, RhythmicTrack, SignalDefect,
        SignalFailureKind, SignalOracleReport, SignalProvider, SyncCase, SyncOracle, TempoRide,
        evaluate_signal, persist_then_assert, write_sync_listening_dump,
    },
};

const TWO_TRACKS: &[RhythmicTrack] = &[
    RhythmicTrack::new(96.0, 220.0),
    RhythmicTrack::new(108.0, 880.0),
];
const FOUR_TRACKS: &[RhythmicTrack] = &[
    RhythmicTrack::new(96.0, 220.0),
    RhythmicTrack::new(108.0, 880.0),
    RhythmicTrack::new(132.0, 1_760.0),
    RhythmicTrack::new(144.0, 3_520.0),
];
const LISTENING_TRACKS: &[RhythmicTrack] = &[
    RhythmicTrack::new(96.0, 220.0).with_burst_of_beat(0.12),
    RhythmicTrack::new(128.0, 440.0)
        .with_burst_of_beat(0.12)
        .with_square_wave(),
];
const LISTENING_SWEEP_TRACKS: &[RhythmicTrack] = &[
    RhythmicTrack::new(110.0, 220.0).with_burst_of_beat(0.12),
    RhythmicTrack::new(130.0, 440.0)
        .with_burst_of_beat(0.12)
        .with_square_wave(),
];
const LISTENING_STEADY: SyncCase = SyncCase::running(
    "legacy-sync-listening-steady",
    48_000,
    OperationOrder::SequentialSync,
    60,
    LISTENING_TRACKS,
)
.with_capture_beats(64);
const LISTENING_SWEEP: SyncCase = SyncCase::running(
    "legacy-sync-listening-sweep",
    48_000,
    OperationOrder::SequentialSync,
    60,
    LISTENING_SWEEP_TRACKS,
)
.with_session_bpm(90.0)
.with_capture_beats(15);
const ASSET: AssetProvider = AssetProvider::new(SignalDefect::None);
const OUT_OF_SYNC: AssetProvider = AssetProvider::new(SignalDefect::OutOfSync);
const PLAYER_QUEUE: PlayerQueueProvider = PlayerQueueProvider;
const ACTIVATION_FRAME_ONLY: &[SignalFailureKind] = &[SignalFailureKind::ActivationFrame];
const BEAT_ORDINAL_ONLY: &[SignalFailureKind] = &[SignalFailureKind::BeatOrdinal];
const BAR_PHASE_ONLY: &[SignalFailureKind] =
    &[SignalFailureKind::BarPhase, SignalFailureKind::BeatOrdinal];
const DRIFT_REQUIRED: &[SignalFailureKind] = &[SignalFailureKind::PostSyncTempo];
const DRIFT_ALLOWED: &[SignalFailureKind] = &[
    SignalFailureKind::PostSyncPhaseSpread,
    SignalFailureKind::PostSyncTempo,
];
const DISCONTINUITY_REQUIRED: &[SignalFailureKind] = &[SignalFailureKind::RhythmicEventLoss];
const DISCONTINUITY_ALLOWED: &[SignalFailureKind] = &[
    SignalFailureKind::Continuity,
    SignalFailureKind::RhythmicEventDivergence,
    SignalFailureKind::RhythmicEventLoss,
];
const OUT_OF_SYNC_REQUIRED: &[SignalFailureKind] = &[SignalFailureKind::PostSyncPhaseSpread];
const OUT_OF_SYNC_ALLOWED: &[SignalFailureKind] = &[SignalFailureKind::PostSyncPhaseSpread];

pub(super) const PLAY_SYNC_SEEK: SyncCase = SyncCase::running(
    "synthetic-play-sync-seek-48k",
    48_000,
    OperationOrder::PlaySyncSeek,
    30,
    TWO_TRACKS,
)
.with_tempo_ride(TempoRide::Triangle, 120);
pub(super) const PLAY_SEEK_SYNC: SyncCase = SyncCase::running(
    "synthetic-play-seek-sync-44k",
    44_100,
    OperationOrder::PlaySeekSync,
    30,
    TWO_TRACKS,
)
.with_tempo_ride(TempoRide::Up, 30);
pub(super) const SEEK_PLAY_SYNC: SyncCase = SyncCase::running(
    "synthetic-seek-play-sync-48k",
    48_000,
    OperationOrder::SeekPlaySync,
    30,
    TWO_TRACKS,
)
.with_tempo_ride(TempoRide::Down, 60);
pub(super) const SEEK_SYNC_PLAY: SyncCase = SyncCase::running(
    "synthetic-seek-sync-play-44k",
    44_100,
    OperationOrder::SeekSyncPlay,
    30,
    TWO_TRACKS,
)
.with_tempo_ride(TempoRide::Triangle, 30);
pub(super) const SYNC_PLAY_SEEK: SyncCase = SyncCase::running(
    "synthetic-sync-play-seek-48k",
    48_000,
    OperationOrder::SyncPlaySeek,
    30,
    TWO_TRACKS,
)
.with_tempo_ride(TempoRide::Up, 60);
pub(super) const SYNC_SEEK_PLAY: SyncCase = SyncCase::running(
    "synthetic-sync-seek-play-44k",
    44_100,
    OperationOrder::SyncSeekPlay,
    30,
    TWO_TRACKS,
)
.with_tempo_ride(TempoRide::Down, 120);
pub(super) const SEQUENTIAL_SYNC: SyncCase = SyncCase::running(
    "synthetic-sequential-sync-48k",
    48_000,
    OperationOrder::SequentialSync,
    30,
    TWO_TRACKS,
)
.with_tempo_ride(TempoRide::Triangle, 60);
pub(super) const PAUSED_SYNC: SyncCase = SyncCase::paused(
    "synthetic-paused-sync-48k",
    48_000,
    OperationOrder::SyncPlaySeek,
    30,
    TWO_TRACKS,
)
.with_tempo_ride(TempoRide::Triangle, 60);
pub(super) const FOUR_DECK_SYNC: SyncCase = SyncCase::running(
    "synthetic-four-deck-sequential-sync-48k",
    48_000,
    OperationOrder::SequentialSync,
    30,
    FOUR_TRACKS,
)
.with_tempo_ride(TempoRide::Triangle, 60);
pub(super) const UP_120: SyncCase = SyncCase::running(
    "synthetic-tempo-up-120hz-48k",
    48_000,
    OperationOrder::PlaySyncSeek,
    30,
    TWO_TRACKS,
)
.with_tempo_ride(TempoRide::Up, 120);
pub(super) const DOWN_30: SyncCase = SyncCase::running(
    "synthetic-tempo-down-30hz-44k",
    44_100,
    OperationOrder::PlaySyncSeek,
    30,
    TWO_TRACKS,
)
.with_tempo_ride(TempoRide::Down, 30);

#[kithara::test(tokio, multi_thread, serial, timeout(Duration::from_secs(600)))]
#[ignore = "SYNC-ORACLE SYNC-PRODUCT-SYN-001..011: waiting for Wave QueueAdapter"]
#[case::play_sync_seek(PLAYER_QUEUE, PLAY_SYNC_SEEK)]
#[case::play_seek_sync(PLAYER_QUEUE, PLAY_SEEK_SYNC)]
#[case::seek_play_sync(PLAYER_QUEUE, SEEK_PLAY_SYNC)]
#[case::seek_sync_play(PLAYER_QUEUE, SEEK_SYNC_PLAY)]
#[case::sync_play_seek(PLAYER_QUEUE, SYNC_PLAY_SEEK)]
#[case::sync_seek_play(PLAYER_QUEUE, SYNC_SEEK_PLAY)]
#[case::sequential_sync(PLAYER_QUEUE, SEQUENTIAL_SYNC)]
#[case::paused_sync(PLAYER_QUEUE, PAUSED_SYNC)]
#[case::four_deck_sync(PLAYER_QUEUE, FOUR_DECK_SYNC)]
#[case::tempo_up_120hz(PLAYER_QUEUE, UP_120)]
#[case::tempo_down_30hz(PLAYER_QUEUE, DOWN_30)]
async fn synthetic_behavioral_matrix_uses_final_pcm_and_cochlea(
    #[case] provider: PlayerQueueProvider,
    #[case] case: SyncCase,
) -> Result<()> {
    let resources = matrix_resources(case, "player-queue")?;
    let bundle = provider
        .capture(case, resources)
        .await?
        .into_player_queue()?;
    let report = SyncOracle::evaluate(case, &bundle);

    persist_then_assert(case, &bundle, &report)
}

#[kithara::test(tokio, multi_thread, serial, timeout(Duration::from_secs(600)))]
#[case::play_sync_seek(ASSET, PLAY_SYNC_SEEK)]
#[case::play_seek_sync(ASSET, PLAY_SEEK_SYNC)]
#[case::seek_play_sync(ASSET, SEEK_PLAY_SYNC)]
#[case::seek_sync_play(ASSET, SEEK_SYNC_PLAY)]
#[case::sync_play_seek(ASSET, SYNC_PLAY_SEEK)]
#[case::sync_seek_play(ASSET, SYNC_SEEK_PLAY)]
#[case::sequential_sync(ASSET, SEQUENTIAL_SYNC)]
#[case::paused_sync(ASSET, PAUSED_SYNC)]
#[case::four_deck_sync(ASSET, FOUR_DECK_SYNC)]
#[case::tempo_up_120hz(ASSET, UP_120)]
#[case::tempo_down_30hz(ASSET, DOWN_30)]
async fn prepared_assets_validate_each_behavioral_oracle_case(
    #[case] provider: AssetProvider,
    #[case] case: SyncCase,
) -> Result<()> {
    let resources = matrix_resources(case, "aligned-asset")?;
    let report = evaluate_signal(provider, case, resources).await?;
    assert!(
        report.is_success(),
        "{}: prepared aligned assets were rejected:\n{}",
        case,
        report
            .failures()
            .iter()
            .map(|failure| failure.message())
            .collect::<Vec<_>>()
            .join("\n")
    );
    Ok(())
}

#[kithara::test(tokio, multi_thread, serial, timeout(Duration::from_secs(600)))]
#[case::play_sync_seek(OUT_OF_SYNC, PLAY_SYNC_SEEK)]
#[case::play_seek_sync(OUT_OF_SYNC, PLAY_SEEK_SYNC)]
#[case::seek_play_sync(OUT_OF_SYNC, SEEK_PLAY_SYNC)]
#[case::seek_sync_play(OUT_OF_SYNC, SEEK_SYNC_PLAY)]
#[case::sync_play_seek(OUT_OF_SYNC, SYNC_PLAY_SEEK)]
#[case::sync_seek_play(OUT_OF_SYNC, SYNC_SEEK_PLAY)]
#[case::sequential_sync(OUT_OF_SYNC, SEQUENTIAL_SYNC)]
#[case::paused_sync(OUT_OF_SYNC, PAUSED_SYNC)]
#[case::four_deck_sync(OUT_OF_SYNC, FOUR_DECK_SYNC)]
#[case::tempo_up_120hz(OUT_OF_SYNC, UP_120)]
#[case::tempo_down_30hz(OUT_OF_SYNC, DOWN_30)]
async fn prepared_unsynced_assets_are_rejected_for_each_behavioral_oracle_case(
    #[case] provider: AssetProvider,
    #[case] case: SyncCase,
) -> Result<()> {
    let resources = matrix_resources(case, "out-of-sync-asset")?;
    let report = evaluate_signal(provider, case, resources).await?;
    assert_rejected_for(case, &report, OUT_OF_SYNC_REQUIRED, OUT_OF_SYNC_ALLOWED);
    Ok(())
}

#[kithara::test(tokio, multi_thread, serial, timeout(Duration::from_secs(600)))]
#[case::one_frame(
    AssetProvider::new(SignalDefect::OneFrame),
    ACTIVATION_FRAME_ONLY,
    ACTIVATION_FRAME_ONLY
)]
#[case::beat_ordinal(
    AssetProvider::new(SignalDefect::BeatOrdinal),
    BEAT_ORDINAL_ONLY,
    BEAT_ORDINAL_ONLY
)]
#[case::bar_phase(
    AssetProvider::new(SignalDefect::BarPhase),
    BAR_PHASE_ONLY,
    BAR_PHASE_ONLY
)]
#[case::drift(AssetProvider::new(SignalDefect::Drift), DRIFT_REQUIRED, DRIFT_ALLOWED)]
#[case::discontinuity(
    AssetProvider::new(SignalDefect::Discontinuity),
    DISCONTINUITY_REQUIRED,
    DISCONTINUITY_ALLOWED
)]
async fn signal_oracle_negative_controls_are_rejected_for_the_intended_reason(
    #[case] provider: AssetProvider,
    #[case] required: &[SignalFailureKind],
    #[case] allowed: &[SignalFailureKind],
) -> Result<()> {
    let resources = matrix_resources(PLAY_SYNC_SEEK, "negative-control")?;
    let report = evaluate_signal(provider, PLAY_SYNC_SEEK, resources).await?;
    assert_rejected_for(PLAY_SYNC_SEEK, &report, required, allowed);
    Ok(())
}

#[kithara::test(tokio, multi_thread, serial, timeout(Duration::from_secs(600)))]
#[ignore = "SYNC-ORACLE waiting for QueueAdapter; then writes five PR150 listening WAVs"]
async fn dump_the_mix_for_listening() -> Result<()> {
    let resources = matrix_resources(LISTENING_STEADY, "listening-dump")?;
    let directory = write_sync_listening_dump(
        resources,
        LISTENING_STEADY,
        LISTENING_SWEEP,
        126.0,
        145.0,
        32,
    )
    .await?
    .expect("KITHARA_AUDIO_ARTIFACT_DIR must be set for the listening dump");
    eprintln!("sync listening WAVs: {}", directory.display());
    Ok(())
}

fn matrix_resources(case: SyncCase, provider: &str) -> Result<SyncFixtureResources> {
    SyncFixtureResources::new(
        &format!("{}-{provider}", case.id),
        BytePool::default(),
        PcmPool::default(),
    )
    .map_err(Into::into)
}

fn assert_rejected_for(
    case: SyncCase,
    report: &SignalOracleReport,
    required: &[SignalFailureKind],
    allowed: &[SignalFailureKind],
) {
    let observed = report.failure_kinds().collect::<BTreeSet<_>>();
    let missing = required
        .iter()
        .copied()
        .filter(|kind| !observed.contains(kind))
        .collect::<Vec<_>>();
    let unexpected = observed
        .iter()
        .copied()
        .filter(|kind| !allowed.contains(kind))
        .collect::<Vec<_>>();

    assert!(
        !report.is_success(),
        "{case}: known-bad signal unexpectedly passed"
    );
    assert!(
        missing.is_empty() && unexpected.is_empty(),
        "{case}: known-bad signal failed for the wrong reason: required={required:?}, observed={observed:?}, unexpected={unexpected:?}, messages={:?}",
        report.failures()
    );
}
