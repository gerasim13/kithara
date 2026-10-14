use kithara::{
    abr::AbrEvent,
    assets::{AssetStore, StorageBackend},
    audio::{AudioConfig, AudioControl, AudioRead, ReadOutcome},
    decode::DecoderBackend,
    events::EventReceiver,
    file::{File, FileConfig},
    hls::{AbrMode, Hls, HlsConfig},
    platform::{
        CancelToken,
        time::Duration,
        tokio::{sync::broadcast::error::TryRecvError, task::spawn_blocking},
    },
    play::{PlayWorker, PlayWorkerConfig},
};
use kithara_integration_tests::{
    TestServerHelper, auto,
    bufpool_ext::{TestPools, pools},
    event::TestEvent,
};
use kithara_test_utils::{TestTempDir, pace};
use url::Url;

/// Stream must continue producing chunks after seek sequence.
///
/// Regression for app3.log: DRM track plays to 23.97s with seeks,
/// then stops producing chunks → `recv_outcome_blocking` hang.
///
/// Parameterized: path × ABR mode to isolate DRM vs HLS vs no-ABR.
pub(crate) async fn stream_continues_after_seek(
    temp_dir: TestTempDir,
    encrypted: bool,
    abr_auto: bool,
    backend: DecoderBackend,
    prepared: (TestServerHelper, Url),
) {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    kithara_integration_tests::apple_warmup::warm_if_apple(backend);

    let (_server, url) = prepared;
    let label = if encrypted { "DRM" } else { "HLS" };

    let cancel = CancelToken::never();
    let pools = pools();
    let worker = PlayWorker::new(
        PlayWorkerConfig::builder(pools.clone())
            .cancel(cancel.clone())
            .build(),
    );
    let abr_mode = if abr_auto {
        auto(0)
    } else {
        AbrMode::manual(0)
    };
    let hls_config = HlsConfig::for_url(url)
        .store(
            AssetStore::builder(pools.clone())
                .backend(StorageBackend::Disk {
                    root: temp_dir.path().to_path_buf(),
                })
                .build(),
        )
        .pools(pools.clone())
        .cancel(cancel)
        .initial_abr_mode(abr_mode)
        .build();

    let config = AudioConfig::<Hls<TestPools>>::for_stream(hls_config)
        .decoder(
            kithara::audio::AudioDecoderConfig::builder()
                .backend(backend)
                .build(),
        )
        .block_on_underrun(true)
        .build();
    let mut audio = worker.load(config).await.expect("create audio");
    // The blocking read phase must NOT run on the test runtime thread: with
    // block_on_underrun the read parks the thread, and on the current-thread
    // runtime that starves the HLS drive/fetch tasks that feed it. preload()
    // primes through the same parking recv, so it belongs here too.
    spawn_blocking(move || {
        let _ = audio.preload();
        let mut buf = vec![0f32; 4096];

        let mut warmup_samples = 0u64;
        while warmup_samples < 17_640 {
            match audio.read(&mut buf) {
                Ok(ReadOutcome::Frames { count, .. }) => warmup_samples += count.get() as u64,
                Ok(ReadOutcome::Pending { .. }) => pace(Duration::from_millis(5)),
                Ok(ReadOutcome::Eof { .. }) => panic!("[{label}] unexpected EOF during warmup"),
                Err(e) => panic!("decode error during warmup: {e}"),
            }
        }

        let samples_per_seek: u64 = 48000 * 2;
        for &target_secs in &[7.0, 13.0, 18.0, 24.0] {
            audio
                .seek(Duration::from_secs_f64(target_secs))
                .expect("seek");

            let mut samples = 0u64;
            while samples < samples_per_seek {
                match audio.read(&mut buf) {
                    Ok(ReadOutcome::Pending { .. }) => {
                        pace(Duration::from_millis(10));
                    }
                    Ok(ReadOutcome::Frames { count, .. }) => {
                        samples += count.get() as u64;
                    }
                    Ok(ReadOutcome::Eof { .. }) => break,
                    Err(e) => panic!("decode error: {e}"),
                }
            }
            assert!(
                samples > 0,
                "[{label}] seek to {target_secs}s must produce samples, got 0"
            );
        }

        let mut post_seek_samples = 0u64;
        while post_seek_samples < samples_per_seek {
            match audio.read(&mut buf) {
                Ok(ReadOutcome::Pending { .. }) => {
                    pace(Duration::from_millis(10));
                }
                Ok(ReadOutcome::Frames { count, .. }) => {
                    post_seek_samples += count.get() as u64;
                }
                Ok(ReadOutcome::Eof { .. }) => break,
                Err(e) => panic!("decode error: {e}"),
            }
        }
        assert!(
            post_seek_samples > 0,
            "[{label}] playback after seeks must continue, got 0 samples"
        );
    })
    .await
    .expect("read phase join");
}

/// Same test but without ABR (fixed variant 0) — baseline.
pub(crate) async fn fixed_variant_on_production_ladder_plays_without_hang(
    temp_dir: TestTempDir,
    mixed_plain: (TestServerHelper, Url),
) {
    let (_server, url) = mixed_plain;

    let cancel = CancelToken::never();
    let pools = pools();
    let worker = PlayWorker::new(
        PlayWorkerConfig::builder(pools.clone())
            .cancel(cancel.clone())
            .build(),
    );
    let hls_config = HlsConfig::for_url(url)
        .store(
            AssetStore::builder(pools.clone())
                .backend(StorageBackend::Disk {
                    root: temp_dir.path().to_path_buf(),
                })
                .build(),
        )
        .pools(pools.clone())
        .cancel(cancel)
        .initial_abr_mode(AbrMode::manual(0))
        .build();

    let config = AudioConfig::<Hls<TestPools>>::for_stream(hls_config)
        .block_on_underrun(true)
        .build();
    let mut audio = worker.load(config).await.expect("create audio");
    spawn_blocking(move || {
        let _ = audio.preload();
        let mut buf = vec![0f32; 4096];
        let mut total_samples = 0u64;

        loop {
            match audio.read(&mut buf) {
                Ok(ReadOutcome::Pending { .. }) => {
                    pace(Duration::from_millis(10));
                }
                Ok(ReadOutcome::Frames { count, .. }) => {
                    total_samples += count.get() as u64;
                    if total_samples >= 100_000 {
                        break;
                    }
                }
                Ok(ReadOutcome::Eof { .. }) => break,
                Err(e) => panic!("decode error: {e}"),
            }
        }

        assert!(
            total_samples > 1000,
            "baseline: expected sustained playback, got only {total_samples} samples"
        );
    })
    .await
    .expect("read phase join");
}

/// Seek after decode-to-EOF in mmap (non-ephemeral) DRM mode must produce samples.
///
/// Regression: after ABR switch + full decode to EOF, random seeks land on
/// segments whose byte offsets are no longer visible in the `StreamIndex` layout,
/// causing `read_at` to return Retry forever.
pub(crate) async fn seek_after_eof_mmap_produces_samples(
    temp_dir: TestTempDir,
    encrypted: bool,
    prepared: (TestServerHelper, Url),
) {
    let (_server, url) = prepared;
    let label = if encrypted { "DRM" } else { "HLS" };

    let cancel = CancelToken::never();
    let pools = pools();
    let worker = PlayWorker::new(
        PlayWorkerConfig::builder(pools.clone())
            .cancel(cancel.clone())
            .build(),
    );
    let hls_config = HlsConfig::for_url(url)
        .store(
            AssetStore::builder(pools.clone())
                .backend(StorageBackend::Disk {
                    root: temp_dir.path().to_path_buf(),
                })
                .build(),
        )
        .pools(pools.clone())
        .cancel(cancel)
        .initial_abr_mode(auto(0))
        .build();

    let config = AudioConfig::<Hls<TestPools>>::for_stream(hls_config)
        .decoder(
            kithara::audio::AudioDecoderConfig::builder()
                .backend(DecoderBackend::default())
                .build(),
        )
        .block_on_underrun(true)
        .build();
    let mut audio = worker.load(config).await.expect("create audio");
    spawn_blocking(move || {
        let _ = audio.preload();
        let mut buf = vec![0f32; 4096];

        let mut warmup_samples = 0u64;
        while warmup_samples < 17_640 {
            match audio.read(&mut buf) {
                Ok(ReadOutcome::Frames { count, .. }) => warmup_samples += count.get() as u64,
                Ok(ReadOutcome::Pending { .. }) => pace(Duration::from_millis(5)),
                Ok(ReadOutcome::Eof { .. }) => panic!("[{label}] unexpected EOF during warmup"),
                Err(e) => panic!("decode error during warmup: {e}"),
            }
        }

        let seek_targets = [
            50.0, 120.0, 5.0, 80.0, 150.0, 30.0, 100.0, 60.0, 140.0, 20.0, 90.0, 10.0, 70.0, 130.0,
            40.0, 110.0,
        ];
        let samples_per_seek: u64 = 48000 * 2;
        for (idx, &target_secs) in seek_targets.iter().enumerate() {
            audio
                .seek(Duration::from_secs_f64(target_secs))
                .unwrap_or_else(|e| panic!("[{label}] seek #{idx} to {target_secs}s failed: {e}"));

            let mut samples = 0u64;
            while samples < samples_per_seek {
                match audio.read(&mut buf) {
                    Ok(ReadOutcome::Pending { .. }) => {
                        pace(Duration::from_millis(10));
                    }
                    Ok(ReadOutcome::Frames { count, .. }) => {
                        samples += count.get() as u64;
                    }
                    Ok(ReadOutcome::Eof { .. }) => break,
                    Err(e) => panic!("decode error: {e}"),
                }
            }
            assert!(
                samples > 0,
                "[{label}] seek #{idx} to {target_secs}s must produce samples, got 0"
            );
        }
    })
    .await
    .expect("read phase join");
}

/// MP3 progressive file must continue producing chunks after seek sequence.
///
/// Same seek pattern as HLS/DRM tests but with `Audio<Stream<File>>`.
/// Baseline: no ABR, no segments, no variant switching.
pub(crate) async fn mp3_stream_continues_after_seek(
    temp_dir: TestTempDir,
    served_mp3: (TestServerHelper, Url),
) {
    let (_server, url) = served_mp3;

    let pools = pools();
    let worker = PlayWorker::new(PlayWorkerConfig::builder(pools.clone()).build());
    let file_config = FileConfig::for_src(url.into())
        .store(
            AssetStore::builder(pools.clone())
                .backend(StorageBackend::Disk {
                    root: temp_dir.path().to_path_buf(),
                })
                .build(),
        )
        .pools(pools.clone())
        .build();
    let config = AudioConfig::<File<TestPools>>::for_stream(file_config)
        .hint(("mp3").to_string())
        .block_on_underrun(true)
        .build();
    let mut audio = worker.load(config).await.expect("create audio");
    spawn_blocking(move || {
        let _ = audio.preload();
        let mut buf = vec![0f32; 4096];

        let mut warmup_samples = 0u64;
        while warmup_samples < 17_640 {
            match audio.read(&mut buf) {
                Ok(ReadOutcome::Frames { count, .. }) => warmup_samples += count.get() as u64,
                Ok(ReadOutcome::Pending { .. }) => pace(Duration::from_millis(5)),
                Ok(ReadOutcome::Eof { .. }) => panic!("[mp3] unexpected EOF during warmup"),
                Err(e) => panic!("decode error during warmup: {e}"),
            }
        }

        let samples_per_seek: u64 = 48000 * 2;
        for &target_secs in &[7.0, 13.0, 18.0, 24.0] {
            audio
                .seek(Duration::from_secs_f64(target_secs))
                .expect("seek");

            let mut samples = 0u64;
            while samples < samples_per_seek {
                match audio.read(&mut buf) {
                    Ok(ReadOutcome::Pending { .. }) => {
                        pace(Duration::from_millis(10));
                    }
                    Ok(ReadOutcome::Frames { count, .. }) => {
                        samples += count.get() as u64;
                    }
                    Ok(ReadOutcome::Eof { .. }) => break,
                    Err(e) => panic!("decode error: {e}"),
                }
            }
            assert!(
                samples > 0,
                "[mp3] seek to {target_secs}s must produce samples, got 0"
            );
        }

        let mut post_seek_samples = 0u64;
        while post_seek_samples < samples_per_seek {
            match audio.read(&mut buf) {
                Ok(ReadOutcome::Pending { .. }) => {
                    pace(Duration::from_millis(10));
                }
                Ok(ReadOutcome::Frames { count, .. }) => {
                    post_seek_samples += count.get() as u64;
                }
                Ok(ReadOutcome::Eof { .. }) => break,
                Err(e) => panic!("decode error: {e}"),
            }
        }
        assert!(
            post_seek_samples > 0,
            "[mp3] playback after seeks must continue, got 0 samples"
        );
    })
    .await
    .expect("read phase join");
}

/// Collect the ABR events that carry a decision, dropping the measurement
/// stream that reports throughput, estimates and buffer depth.
pub(crate) fn drain_abr_decisions(events: &mut EventReceiver<TestEvent>, sink: &mut Vec<AbrEvent>) {
    loop {
        match events.try_recv() {
            Ok(envelope) => {
                if let TestEvent::Abr(event) = envelope.event
                    && !matches!(
                        event,
                        AbrEvent::ThroughputSample { .. }
                            | AbrEvent::BandwidthEstimate { .. }
                            | AbrEvent::BufferAhead { .. }
                    )
                {
                    sink.push(event);
                }
            }
            Err(TryRecvError::Lagged(_)) => {}
            Err(TryRecvError::Empty | TryRecvError::Closed) => return,
        }
    }
}

pub(crate) fn lowest_pending_estimate(events: &mut EventReceiver<TestEvent>) -> Option<u64> {
    let mut lowest: Option<u64> = None;
    loop {
        match events.try_recv() {
            Ok(envelope) => {
                if let TestEvent::Abr(AbrEvent::BandwidthEstimate { bps }) = envelope.event {
                    lowest = Some(lowest.map_or(bps, |low| low.min(bps)));
                }
            }
            Err(TryRecvError::Lagged(_)) => {}
            Err(TryRecvError::Empty | TryRecvError::Closed) => return lowest,
        }
    }
}
