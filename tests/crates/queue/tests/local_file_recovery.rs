#![cfg(not(target_arch = "wasm32"))]

use kithara::{
    platform::time::{Duration, WallInstant},
    queue::{QueueControl, TrackSource, TrackStatus},
};
use kithara_integration_tests::{
    kithara,
    offline::{OfflinePlayer, OfflinePlayerOptions},
};
use kithara_test_fixtures::assets;
use kithara_test_utils::TestTempDir;

#[kithara::test(native, tokio)]
#[kithara::flash(io)]
#[kithara_test_utils::kithara::hang_watchdog(timeout = Duration::from_secs(1))]
async fn an_absent_local_file_recovers_only_after_an_explicit_play() {
    let directory = TestTempDir::new();
    let path = directory.path().join("retry.wav");
    assert!(path.is_absolute());
    assert!(!path.exists());
    let source = path.to_str().expect("UTF-8 fixture path").to_owned();
    let harness = OfflinePlayer::with_sample_rate(
        OfflinePlayerOptions::builder()
            .crossfade_duration(0.0)
            .block_on_underrun(true)
            .build(),
        44_100,
    )
    .await;
    let queue = harness.player().clone();
    let expected_source = source.clone();
    let id = harness
        .run(&queue, move |queue| queue.append(TrackSource::Uri(source)))
        .await
        .expect("append the absent absolute source");
    loop {
        hang_tick!();
        harness.render(128).await;
        if matches!(
            queue.track(id).expect("retained source").status,
            TrackStatus::Failed(_)
        ) {
            break;
        }
    }
    hang_reset!();
    assert_eq!(
        queue.track(id).expect("retained source").url.as_deref(),
        Some(expected_source.as_str())
    );
    assert!(queue.current().is_none());
    {
        let _io = kithara::platform::flash::real_io();
        std::fs::copy(
            assets::fixed_constant_wav_cd_half_44100()
                .path()
                .expect("prepared WAV"),
            &path,
        )
        .expect("restore the same requested file");
    }
    for _ in 0..8 {
        hang_tick!();
        let output = harness.render(128).await;
        assert!(output.iter().all(|sample| *sample == 0.0));
        assert!(matches!(
            queue.track(id).expect("retained source").status,
            TrackStatus::Failed(_)
        ));
        assert!(queue.current().is_none());
    }
    hang_reset!();
    harness.run(&queue, QueueControl::play).await;
    harness
        .render_until_current(id, 128, WallInstant::now() + Duration::from_secs(5))
        .await;
    let mut audible = false;
    for _ in 0..16 {
        hang_tick!();
        let output = harness.render(128).await;
        assert_eq!(output.len(), 256);
        assert!(output.iter().all(|sample| sample.is_finite()));
        audible |= output.iter().any(|sample| sample.abs() > 0.1);
        if audible {
            break;
        }
    }
    assert!(audible, "the restored file must supply actual output PCM");
    assert_eq!(queue.current().map(|entry| entry.id), Some(id));
    assert_eq!(
        queue.track(id).expect("retained source").url.as_deref(),
        Some(expected_source.as_str())
    );
    harness.close().await;
}
