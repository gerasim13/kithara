#![cfg(not(target_arch = "wasm32"))]

use anyhow::{Context, Result};
use kithara::platform::{CancelToken, time::Duration};
use kithara_integration_tests::{
    kithara, rt_cancel,
    sync_fixture::{SyncAnalysisFixtures, library_pair_from_env},
    sync_matrix::assert_behavioral_row,
};

use super::{
    sync_behavioral_matrix::{
        DOWN_30, FOUR_DECK_SYNC, PAUSED_SYNC, PLAY_SEEK_SYNC, PLAY_SYNC_SEEK, SEEK_PLAY_SYNC,
        SEEK_SYNC_PLAY, SEQUENTIAL_SYNC, SYNC_PLAY_SEEK, SYNC_SEEK_PLAY, UP_120,
    },
    sync_media::analyzed_media,
};

const LIBRARY_ROW_ID: &str = "media-library-distinct-opt-in";

#[kithara::test(tokio, multi_thread, serial, timeout(Duration::from_secs(600)))]
#[ignore = "SYNC-ORACLE SYNC-PRODUCT-LIB-001..011: waiting for Wave Acceptance and KITHARA_SYNC_LIBRARY"]
#[case::play_sync_seek(PLAY_SYNC_SEEK)]
#[case::play_seek_sync(PLAY_SEEK_SYNC)]
#[case::seek_play_sync(SEEK_PLAY_SYNC)]
#[case::seek_sync_play(SEEK_SYNC_PLAY)]
#[case::sync_play_seek(SYNC_PLAY_SEEK)]
#[case::sync_seek_play(SYNC_SEEK_PLAY)]
#[case::sequential(SEQUENTIAL_SYNC)]
#[case::paused(PAUSED_SYNC)]
#[case::four_decks(FOUR_DECK_SYNC)]
#[case::tempo_up_120(UP_120)]
#[case::tempo_down_30(DOWN_30)]
async fn opt_in_library_pair_runs_the_full_behavioral_row(
    rt_cancel: CancelToken,
    #[case] case: kithara_integration_tests::sync_matrix::SyncCase,
) -> Result<()> {
    let pair = library_pair_from_env()
        .await
        .context("resolve explicitly configured sync music library")?
        .context("ignored library matrix requires KITHARA_SYNC_LIBRARY")?;
    let analysis = SyncAnalysisFixtures::production()
        .context("initialize production analysis for opt-in library")?;
    let mut media = analyzed_media(
        LIBRARY_ROW_ID,
        &analysis,
        &rt_cancel,
        vec![
            ("deck-a", pair.deck_a().clone()),
            ("deck-b", pair.deck_b().clone()),
        ],
    )
    .await?;
    if let Some(seed) = pair.library_seed() {
        media = media.with_library_seed(seed);
    }

    let _report = assert_behavioral_row(case, media)
        .await
        .with_context(|| format!("run opt-in library behavioral case {}", case.id))?;
    Ok(())
}
