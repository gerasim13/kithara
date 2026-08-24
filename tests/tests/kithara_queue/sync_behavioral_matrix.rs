#![cfg(not(target_arch = "wasm32"))]

use anyhow::Result;
use kithara::platform::time::Duration;
use kithara_integration_tests::{
    kithara,
    sync_matrix::{
        OperationOrder, SyncCase, SyncOracle, TempoRide, persist_then_assert,
        run_synthetic_behavioral_row,
    },
};

pub(super) const PLAY_SYNC_SEEK: SyncCase = SyncCase::running(
    "synthetic-play-sync-seek-48k",
    2,
    48_000,
    OperationOrder::PlaySyncSeek,
)
.with_tempo_ride(TempoRide::Triangle, 120);
pub(super) const PLAY_SEEK_SYNC: SyncCase = SyncCase::running(
    "synthetic-play-seek-sync-44k",
    2,
    44_100,
    OperationOrder::PlaySeekSync,
)
.with_tempo_ride(TempoRide::Up, 30);
pub(super) const SEEK_PLAY_SYNC: SyncCase = SyncCase::running(
    "synthetic-seek-play-sync-48k",
    2,
    48_000,
    OperationOrder::SeekPlaySync,
)
.with_tempo_ride(TempoRide::Down, 60);
pub(super) const SEEK_SYNC_PLAY: SyncCase = SyncCase::running(
    "synthetic-seek-sync-play-44k",
    2,
    44_100,
    OperationOrder::SeekSyncPlay,
)
.with_tempo_ride(TempoRide::Triangle, 30);
pub(super) const SYNC_PLAY_SEEK: SyncCase = SyncCase::running(
    "synthetic-sync-play-seek-48k",
    2,
    48_000,
    OperationOrder::SyncPlaySeek,
)
.with_tempo_ride(TempoRide::Up, 60);
pub(super) const SYNC_SEEK_PLAY: SyncCase = SyncCase::running(
    "synthetic-sync-seek-play-44k",
    2,
    44_100,
    OperationOrder::SyncSeekPlay,
)
.with_tempo_ride(TempoRide::Down, 120);
pub(super) const SEQUENTIAL_SYNC: SyncCase = SyncCase::running(
    "synthetic-sequential-sync-48k",
    2,
    48_000,
    OperationOrder::SequentialSync,
)
.with_tempo_ride(TempoRide::Triangle, 60);
pub(super) const PAUSED_SYNC: SyncCase = SyncCase::paused(
    "synthetic-paused-sync-48k",
    2,
    48_000,
    OperationOrder::SyncPlaySeek,
)
.with_tempo_ride(TempoRide::Triangle, 60);
pub(super) const FOUR_DECK_SYNC: SyncCase = SyncCase::running(
    "synthetic-four-deck-sequential-sync-48k",
    4,
    48_000,
    OperationOrder::SequentialSync,
)
.with_tempo_ride(TempoRide::Triangle, 60);
pub(super) const UP_120: SyncCase = SyncCase::running(
    "synthetic-tempo-up-120hz-48k",
    2,
    48_000,
    OperationOrder::PlaySyncSeek,
)
.with_tempo_ride(TempoRide::Up, 120);
pub(super) const DOWN_30: SyncCase = SyncCase::running(
    "synthetic-tempo-down-30hz-44k",
    2,
    44_100,
    OperationOrder::PlaySyncSeek,
)
.with_tempo_ride(TempoRide::Down, 30);

#[kithara::test(tokio, multi_thread, serial, timeout(Duration::from_secs(600)))]
#[ignore = "SYNC-ORACLE SYNC-PRODUCT-SYN-001..011: waiting for Wave QueueAdapter"]
#[case::play_sync_seek(PLAY_SYNC_SEEK)]
#[case::play_seek_sync(PLAY_SEEK_SYNC)]
#[case::seek_play_sync(SEEK_PLAY_SYNC)]
#[case::seek_sync_play(SEEK_SYNC_PLAY)]
#[case::sync_play_seek(SYNC_PLAY_SEEK)]
#[case::sync_seek_play(SYNC_SEEK_PLAY)]
#[case::sequential_sync(SEQUENTIAL_SYNC)]
#[case::paused_sync(PAUSED_SYNC)]
#[case::four_deck_sync(FOUR_DECK_SYNC)]
#[case::tempo_up_120hz(UP_120)]
#[case::tempo_down_30hz(DOWN_30)]
async fn synthetic_behavioral_matrix_uses_final_pcm_and_cochlea(
    #[case] case: SyncCase,
) -> Result<()> {
    let bundle = run_synthetic_behavioral_row(case).await?;
    let report = SyncOracle::evaluate(case, &bundle);

    persist_then_assert(case, &bundle, &report)
}
