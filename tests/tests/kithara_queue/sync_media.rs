#![cfg(not(target_arch = "wasm32"))]

use anyhow::{Context, Result};
use kithara::{
    bufpool::{BytePool, PcmPool},
    hls::AbrMode,
    platform::{CancelToken, time::Duration},
    play::ResourceConfig,
};
use kithara_integration_tests::{
    TestServerHelper, kithara, memory_asset_store, rt_cancel,
    sync_fixture::{
        RepositoryMp3, SyncAnalysisFixtures, SyncTrackFixture as AnalysisTrackFixture,
        repository_mp3, repository_mp3_pair, silvercomet_hls,
    },
    sync_matrix::{SyncMedia, SyncTrackFixture, assert_behavioral_row},
};

use super::sync_behavioral_matrix::{
    DOWN_30, FOUR_DECK_SYNC, PAUSED_SYNC, PLAY_SEEK_SYNC, PLAY_SYNC_SEEK, SEEK_PLAY_SYNC,
    SEEK_SYNC_PLAY, SEQUENTIAL_SYNC, SYNC_PLAY_SEEK, SYNC_SEEK_PLAY, UP_120,
};

#[derive(Clone, Copy, Debug)]
enum MediaKind {
    HlsSame,
    HlsWithMp3,
    Mp3Distinct,
    Mp3Same,
}

#[derive(Clone, Copy, Debug)]
struct MediaRow {
    id: &'static str,
    kind: MediaKind,
}

const HLS_SAME: MediaRow = MediaRow {
    id: "media-hls-same-independent",
    kind: MediaKind::HlsSame,
};
const MP3_SAME: MediaRow = MediaRow {
    id: "media-mp3-same-independent",
    kind: MediaKind::Mp3Same,
};
const MP3_DISTINCT: MediaRow = MediaRow {
    id: "media-mp3-distinct",
    kind: MediaKind::Mp3Distinct,
};
const HLS_WITH_MP3: MediaRow = MediaRow {
    id: "media-hls-with-mp3",
    kind: MediaKind::HlsWithMp3,
};

#[kithara::test(tokio, multi_thread, serial, timeout(Duration::from_secs(600)))]
#[ignore = "SYNC-ORACLE SYNC-PRODUCT-MEDIA-001..044: waiting for Wave QueueAdapter"]
#[case::hls_same_play_sync_seek(HLS_SAME, PLAY_SYNC_SEEK)]
#[case::hls_same_play_seek_sync(HLS_SAME, PLAY_SEEK_SYNC)]
#[case::hls_same_seek_play_sync(HLS_SAME, SEEK_PLAY_SYNC)]
#[case::hls_same_seek_sync_play(HLS_SAME, SEEK_SYNC_PLAY)]
#[case::hls_same_sync_play_seek(HLS_SAME, SYNC_PLAY_SEEK)]
#[case::hls_same_sync_seek_play(HLS_SAME, SYNC_SEEK_PLAY)]
#[case::hls_same_sequential(HLS_SAME, SEQUENTIAL_SYNC)]
#[case::hls_same_paused(HLS_SAME, PAUSED_SYNC)]
#[case::hls_same_four_decks(HLS_SAME, FOUR_DECK_SYNC)]
#[case::hls_same_tempo_up_120(HLS_SAME, UP_120)]
#[case::hls_same_tempo_down_30(HLS_SAME, DOWN_30)]
#[case::mp3_same_play_sync_seek(MP3_SAME, PLAY_SYNC_SEEK)]
#[case::mp3_same_play_seek_sync(MP3_SAME, PLAY_SEEK_SYNC)]
#[case::mp3_same_seek_play_sync(MP3_SAME, SEEK_PLAY_SYNC)]
#[case::mp3_same_seek_sync_play(MP3_SAME, SEEK_SYNC_PLAY)]
#[case::mp3_same_sync_play_seek(MP3_SAME, SYNC_PLAY_SEEK)]
#[case::mp3_same_sync_seek_play(MP3_SAME, SYNC_SEEK_PLAY)]
#[case::mp3_same_sequential(MP3_SAME, SEQUENTIAL_SYNC)]
#[case::mp3_same_paused(MP3_SAME, PAUSED_SYNC)]
#[case::mp3_same_four_decks(MP3_SAME, FOUR_DECK_SYNC)]
#[case::mp3_same_tempo_up_120(MP3_SAME, UP_120)]
#[case::mp3_same_tempo_down_30(MP3_SAME, DOWN_30)]
#[case::mp3_distinct_play_sync_seek(MP3_DISTINCT, PLAY_SYNC_SEEK)]
#[case::mp3_distinct_play_seek_sync(MP3_DISTINCT, PLAY_SEEK_SYNC)]
#[case::mp3_distinct_seek_play_sync(MP3_DISTINCT, SEEK_PLAY_SYNC)]
#[case::mp3_distinct_seek_sync_play(MP3_DISTINCT, SEEK_SYNC_PLAY)]
#[case::mp3_distinct_sync_play_seek(MP3_DISTINCT, SYNC_PLAY_SEEK)]
#[case::mp3_distinct_sync_seek_play(MP3_DISTINCT, SYNC_SEEK_PLAY)]
#[case::mp3_distinct_sequential(MP3_DISTINCT, SEQUENTIAL_SYNC)]
#[case::mp3_distinct_paused(MP3_DISTINCT, PAUSED_SYNC)]
#[case::mp3_distinct_four_decks(MP3_DISTINCT, FOUR_DECK_SYNC)]
#[case::mp3_distinct_tempo_up_120(MP3_DISTINCT, UP_120)]
#[case::mp3_distinct_tempo_down_30(MP3_DISTINCT, DOWN_30)]
#[case::hls_mp3_play_sync_seek(HLS_WITH_MP3, PLAY_SYNC_SEEK)]
#[case::hls_mp3_play_seek_sync(HLS_WITH_MP3, PLAY_SEEK_SYNC)]
#[case::hls_mp3_seek_play_sync(HLS_WITH_MP3, SEEK_PLAY_SYNC)]
#[case::hls_mp3_seek_sync_play(HLS_WITH_MP3, SEEK_SYNC_PLAY)]
#[case::hls_mp3_sync_play_seek(HLS_WITH_MP3, SYNC_PLAY_SEEK)]
#[case::hls_mp3_sync_seek_play(HLS_WITH_MP3, SYNC_SEEK_PLAY)]
#[case::hls_mp3_sequential(HLS_WITH_MP3, SEQUENTIAL_SYNC)]
#[case::hls_mp3_paused(HLS_WITH_MP3, PAUSED_SYNC)]
#[case::hls_mp3_four_decks(HLS_WITH_MP3, FOUR_DECK_SYNC)]
#[case::hls_mp3_tempo_up_120(HLS_WITH_MP3, UP_120)]
#[case::hls_mp3_tempo_down_30(HLS_WITH_MP3, DOWN_30)]
async fn media_source_axis_runs_the_full_behavioral_row(
    rt_cancel: CancelToken,
    #[case] row: MediaRow,
    #[case] case: kithara_integration_tests::sync_matrix::SyncCase,
) -> Result<()> {
    let server = TestServerHelper::new().await;
    let analysis = SyncAnalysisFixtures::production()
        .with_context(|| format!("{}: initialize production analysis", row.id))?;
    let inputs = media_inputs(row, &server).await?;
    let media = analyzed_media(row.id, &analysis, &rt_cancel, inputs).await?;
    let _report = assert_behavioral_row(case, media)
        .await
        .with_context(|| format!("{}: run full behavioral case {}", row.id, case.id))?;
    Ok(())
}

async fn media_inputs(
    row: MediaRow,
    server: &TestServerHelper,
) -> Result<Vec<(&'static str, AnalysisTrackFixture)>> {
    let inputs = match row.kind {
        MediaKind::HlsSame => vec![
            ("deck-a", silvercomet_hls(server).await?),
            ("deck-b", silvercomet_hls(server).await?),
        ],
        MediaKind::Mp3Same => vec![
            ("deck-a", repository_mp3(server, RepositoryMp3::Test).await?),
            ("deck-b", repository_mp3(server, RepositoryMp3::Test).await?),
        ],
        MediaKind::Mp3Distinct => {
            let pair = repository_mp3_pair(server).await?;
            vec![
                ("deck-a", pair.deck_a().clone()),
                ("deck-b", pair.deck_b().clone()),
            ]
        }
        MediaKind::HlsWithMp3 => vec![
            ("deck-a", silvercomet_hls(server).await?),
            ("deck-b", repository_mp3(server, RepositoryMp3::Test).await?),
        ],
    };
    Ok(inputs)
}

pub(super) async fn analyzed_media(
    id: &'static str,
    analysis: &SyncAnalysisFixtures,
    cancel: &CancelToken,
    inputs: Vec<(&'static str, AnalysisTrackFixture)>,
) -> Result<SyncMedia> {
    let mut tracks = Vec::with_capacity(inputs.len());
    for (deck, input) in inputs {
        let cached = analysis
            .analyze(cancel, &input)
            .await
            .with_context(|| format!("{id}: analyze {deck} source '{}'", input.media()))?;
        let analysis_key = cached.key().to_owned();
        let playback = ResourceConfig::for_src(input.source().clone())
            .initial_abr_mode(AbrMode::manual(0))
            .store(memory_asset_store())
            .byte_pool(BytePool::default())
            .pcm_pool(PcmPool::default())
            .build();
        let track = SyncTrackFixture::new(
            format!("{deck}:{}", input.media()),
            playback,
            cached.into_analysis(),
            analysis_key,
        );
        tracks.push(if input.is_hls() {
            track.with_abr_target(1)
        } else {
            track
        });
    }
    Ok(SyncMedia::new(id, tracks))
}
