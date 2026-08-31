#![expect(
    clippy::cast_precision_loss,
    reason = "RSS values in MB, f64 precision is sufficient"
)]

use hotpath::HotpathGuardBuilder;
use kithara::{
    assets::{AssetStore, StorageBackend},
    audio::{AudioConfig, AudioRead, DecodeError, ReadOutcome},
    bufpool::Region,
    hls::{Hls, HlsConfig},
    platform::{
        time::{Duration, Instant},
        tokio::task::spawn_blocking,
    },
    play::{PlayWorker, PlayWorkerConfig},
};
use kithara_integration_tests::{TestServerHelper, TestTempDir, auto, temp_dir};
use memory_stats::memory_stats;
use tracing::info;

struct Consts;
impl Consts {
    const MB: usize = 1024 * 1024;
    const BUDGET_RUNS: usize = 3;
    const READ_FRAMES: usize = 4096;
    /// Upper bound on one drain. Nothing paces the reader, so a healthy drain
    /// ends far below this; it is here so a stalled stream fails the test
    /// instead of hanging it.
    const DRAIN_LIMIT: Duration = Duration::from_secs(20);
    /// Share of a drain that counts as warmup. Measured 2026-08-31: RSS climbs
    /// from 43.7 MB to 47.0 MB inside the first tenth of the reads and is flat
    /// for the remaining nine, so a quarter clears the ramp with room to
    /// spare.
    const WARMUP_SHARE: usize = 4;
    const RSS_BUDGET_MB: usize = 30;
    const LEAK_TOLERANCE_MB: usize = 5;
}

/// Why a drain stopped.
///
/// Only [`Self::Eof`] leaves a complete measurement behind. The other two
/// truncate it, and a truncated drain cannot say whether RSS settled.
#[derive(Debug)]
enum DrainEnd {
    Eof,
    Failed(DecodeError),
    Deadline,
}

/// RSS along one read of a stream to its end, and how that read finished.
struct Drain {
    samples: Vec<usize>,
    end: DrainEnd,
    elapsed: Duration,
}

impl Drain {
    /// RSS samples of a drain that reached the end of the stream.
    ///
    /// Panics on any other ending. A drain that stops early still produces
    /// numbers, and those numbers agree with any budget, so scoring one would
    /// leave the assertions below unable to fail.
    fn complete_samples(&self) -> &[usize] {
        assert!(
            matches!(self.end, DrainEnd::Eof),
            "drain stopped short of the end of the stream after {:?} and {} reads: {:?}",
            self.elapsed,
            self.samples.len(),
            self.end,
        );
        assert!(
            self.samples.len() >= Consts::WARMUP_SHARE,
            "drain produced {} reads, too few to tell warmup from the rest",
            self.samples.len(),
        );
        &self.samples
    }
}

/// Reads `audio` to the end of the stream, sampling RSS after every read.
///
/// The reader is not paced against a clock, so the measurement window is the
/// drain itself rather than any wall-clock span: the whole track comes out in
/// a few seconds. That is why the samples are indexed by read below and not by
/// elapsed time.
fn drain_sampling_rss<A: AudioRead>(audio: &mut A) -> Drain {
    let mut buf = vec![0f32; Consts::READ_FRAMES];
    let mut samples = Vec::new();
    let start = Instant::now();

    let end = loop {
        if start.elapsed() >= Consts::DRAIN_LIMIT {
            break DrainEnd::Deadline;
        }
        match audio.read(&mut buf) {
            Ok(ReadOutcome::Eof { .. }) => break DrainEnd::Eof,
            Ok(_) => {}
            Err(error) => break DrainEnd::Failed(error),
        }
        if let Some(stats) = memory_stats() {
            samples.push(stats.physical_mem);
        }
    };

    Drain {
        samples,
        end,
        elapsed: start.elapsed(),
    }
}

/// Multi-run RSS measurement: peak RSS delta must stay within budget.
#[kithara::test(
    native,
    tokio,
    serial,
    timeout(Duration::from_secs(30)),
    hang_timeout_secs(5)
)]
async fn test_hls_playback_rss_within_budget(temp_dir: TestTempDir) {
    let _guard = HotpathGuardBuilder::new("rss_budget").build();
    let mut run_deltas = Vec::with_capacity(Consts::BUDGET_RUNS);

    for run in 0..Consts::BUDGET_RUNS {
        let baseline_rss = memory_stats()
            .expect("memory_stats unsupported")
            .physical_mem;

        let server = TestServerHelper::new().await;
        let url = server.asset("hls/master.m3u8");

        let region = Region::default();
        let byte_pool = region.byte_pool();
        let store = AssetStore::builder()
            .backend(StorageBackend::Disk {
                root: temp_dir.path().into(),
            })
            .pool(byte_pool.clone())
            .build();
        let hls_config = HlsConfig::for_url(url)
            .store(store)
            .pool(byte_pool.clone())
            .initial_abr_mode(auto(0))
            .build();
        let config = AudioConfig::<Hls>::for_stream(hls_config).build();
        let worker =
            PlayWorker::new(PlayWorkerConfig::for_pools(byte_pool, region.sample_pool()).build());
        let mut audio = worker.open(config).await.expect("audio creation");

        let drain = spawn_blocking(move || drain_sampling_rss(&mut audio))
            .await
            .expect("spawn_blocking");

        let samples = drain.complete_samples();
        let peak_rss = samples
            .iter()
            .copied()
            .max()
            .expect("a complete drain has samples");
        let delta = peak_rss.saturating_sub(baseline_rss);
        run_deltas.push(delta);

        info!(
            "Run {run}: baseline={:.1}MB peak={:.1}MB delta={:.1}MB reads={} elapsed={:?}",
            baseline_rss as f64 / Consts::MB as f64,
            peak_rss as f64 / Consts::MB as f64,
            delta as f64 / Consts::MB as f64,
            samples.len(),
            drain.elapsed,
        );

        drop(server);
    }

    let min_delta = run_deltas.iter().copied().min().unwrap_or(0);
    let max_delta = run_deltas.iter().copied().max().unwrap_or(0);
    let mean_delta = run_deltas.iter().sum::<usize>() / run_deltas.len();

    info!(
        "RSS deltas: min={:.1}MB mean={:.1}MB max={:.1}MB budget={}MB",
        min_delta as f64 / Consts::MB as f64,
        mean_delta as f64 / Consts::MB as f64,
        max_delta as f64 / Consts::MB as f64,
        Consts::RSS_BUDGET_MB
    );

    assert!(
        max_delta < Consts::RSS_BUDGET_MB * Consts::MB,
        "RSS exceeded budget: max delta {:.1}MB > {}MB",
        max_delta as f64 / Consts::MB as f64,
        Consts::RSS_BUDGET_MB
    );
}

/// RSS should stabilize after warmup — no sustained growth.
#[kithara::test(
    native,
    tokio,
    serial,
    timeout(Duration::from_secs(30)),
    hang_timeout_secs(5)
)]
async fn test_hls_playback_no_rss_leak(temp_dir: TestTempDir) {
    let _guard = HotpathGuardBuilder::new("rss_leak").build();
    let server = TestServerHelper::new().await;
    let url = server.asset("hls/master.m3u8");

    let region = Region::default();
    let byte_pool = region.byte_pool();
    let store = AssetStore::builder()
        .backend(StorageBackend::Disk {
            root: temp_dir.path().into(),
        })
        .pool(byte_pool.clone())
        .build();
    let hls_config = HlsConfig::for_url(url)
        .store(store)
        .pool(byte_pool.clone())
        .initial_abr_mode(auto(0))
        .build();
    let config = AudioConfig::<Hls>::for_stream(hls_config).build();
    let worker =
        PlayWorker::new(PlayWorkerConfig::for_pools(byte_pool, region.sample_pool()).build());
    let mut audio = worker.open(config).await.expect("audio creation");

    let drain = spawn_blocking(move || drain_sampling_rss(&mut audio))
        .await
        .expect("spawn_blocking");

    let samples = drain.complete_samples();
    let warmup_reads = samples.len() / Consts::WARMUP_SHARE;
    let warmup_rss = samples[..warmup_reads]
        .iter()
        .copied()
        .max()
        .expect("a complete drain has a warmup share");
    let final_rss = *samples.last().expect("a complete drain has samples");
    let growth = final_rss.saturating_sub(warmup_rss);

    info!(
        "Leak test: warmup={:.1}MB final={:.1}MB growth={:.1}MB tolerance={}MB \
         reads={} warmup_reads={warmup_reads}",
        warmup_rss as f64 / Consts::MB as f64,
        final_rss as f64 / Consts::MB as f64,
        growth as f64 / Consts::MB as f64,
        Consts::LEAK_TOLERANCE_MB,
        samples.len(),
    );

    assert!(
        growth < Consts::LEAK_TOLERANCE_MB * Consts::MB,
        "RSS grew after warmup: {:.1}MB > {}MB (warmup={:.1}MB final={:.1}MB)",
        growth as f64 / Consts::MB as f64,
        Consts::LEAK_TOLERANCE_MB,
        warmup_rss as f64 / Consts::MB as f64,
        final_rss as f64 / Consts::MB as f64,
    );
}
