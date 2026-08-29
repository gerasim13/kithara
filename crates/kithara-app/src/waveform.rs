use std::num::NonZeroU32;

pub use kithara::analysis::TrackAnalysis;
use kithara::{
    analysis::{
        AnalysisFingerprint, AnalysisPass, AnalysisProducer, AnalysisToken, AnalysisWorker,
        AnalyzerBuilder, BeatAnalysisConfig,
    },
    audio::AudioReader,
    bufpool::SamplePool,
    prelude::{PlaybackResamplerBackend, Resource, ResourceConfig},
};
use kithara_platform::{
    CancelToken,
    sync::Arc,
    tokio::{
        sync::watch,
        task::{self, JoinHandle},
    },
};
use tracing::warn;

type AppAnalysisWorker = AnalysisWorker<PlaybackResamplerBackend>;
type AppBeatAnalysisConfig = BeatAnalysisConfig<PlaybackResamplerBackend>;
type AppResourceConfig = ResourceConfig<PlaybackResamplerBackend>;

/// App-side handle over the shared [`AnalysisWorker`]: opens the resource
/// off the player runtime, hands the opened reader to the worker thread,
/// and keeps at most one run in flight. Dropping it cancels the run and
/// stops the worker.
#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub struct TrackAnalysisRunner {
    worker: Arc<AppAnalysisWorker>,
    current: Option<RunHandle>,
    /// What this configuration produces, per artifact: the cache keys off it.
    fingerprint: AnalysisFingerprint,
    /// Whether any analyzer is compiled in; without one a decode pass would
    /// produce nothing, so the driver skips analysis entirely.
    #[field(get = is_active)]
    active: bool,
}

/// An in-flight run: its child token and the spawned fallback-reader open task.
/// Teardown is cooperative — cancelling the token exits the worker's decode
/// loop at its next per-chunk check.
struct RunHandle {
    cancel: CancelToken,
    task: JoinHandle<()>,
}

impl TrackAnalysisRunner {
    /// `master` must be a child of the app master cancel; the worker thread
    /// and every run scope live under it. `buckets` caps the waveform output;
    /// the native window count is the real resolution.
    #[must_use]
    pub fn new(
        master: &CancelToken,
        _buckets: usize,
        beat_config: AppBeatAnalysisConfig,
        sample_pool: SamplePool,
    ) -> Self {
        let builder = AnalyzerBuilder::new(sample_pool).with_beat_config(beat_config);
        #[cfg(feature = "analysis-waveform")]
        let builder = builder.with_waveform(_buckets);
        let builder = builder.with_beat();
        let worker = Arc::new(AnalysisWorker::new(master, builder));
        let active = worker.is_active();
        let fingerprint = worker.fingerprint().clone();
        Self {
            fingerprint,
            worker,
            active,
            current: None,
        }
    }

    /// What the active configuration produces, per artifact.
    #[must_use]
    pub const fn fingerprint(&self) -> &AnalysisFingerprint {
        &self.fingerprint
    }

    /// Cancel any prior run and queue `config` for analysis on the `rate`
    /// axis: the reader is opened onto it and the pass is measured in it, so
    /// a producer feeding the same pass later shares one axis with it.
    /// Staged results arrive on the returned receiver,
    /// which closes when the run ends; nothing arrives on failure/cancel.
    /// `deliver` receives the producer half synchronously, before the fallback
    /// reader is opened. The runner does not know what the handle is for;
    /// attaching it to the track's playback path is the caller's business.
    pub fn analyze<D>(
        &mut self,
        config: AppResourceConfig,
        token: AnalysisToken,
        rate: NonZeroU32,
        deliver: D,
    ) -> watch::Receiver<Option<TrackAnalysis>>
    where
        D: FnOnce(AnalysisProducer),
    {
        self.clear();

        let (rx, producer, pass) = self.worker.open(token, rate);
        let run = pass.cancel_token().clone();
        deliver(producer);
        let task = task::spawn(run_analysis(
            Arc::clone(&self.worker),
            config,
            run.clone(),
            rate,
            pass,
        ));
        self.current = Some(RunHandle { task, cancel: run });
        rx
    }

    /// Cancel the in-flight run.
    pub fn clear(&mut self) {
        if let Some(prev) = self.current.take() {
            prev.cancel.cancel();
            prev.task.abort();
        }
    }
}

impl Drop for TrackAnalysisRunner {
    fn drop(&mut self) {
        self.clear();
    }
}

/// Open `config` and start the already-open pass on the shared worker.
async fn run_analysis(
    worker: Arc<AppAnalysisWorker>,
    config: AppResourceConfig,
    cancel: CancelToken,
    rate: NonZeroU32,
    pass: AnalysisPass,
) {
    let Some(reader) = open_reader(config, &cancel, rate).await else {
        return;
    };
    worker.start(pass, reader);
}

/// Open the resource under the run's cancel scope (so preemption and app
/// shutdown tear its registered playback task down top-down) and unwrap the
/// reader for the analysis worker.
async fn open_reader(
    mut config: AppResourceConfig,
    cancel: &CancelToken,
    rate: NonZeroU32,
) -> Option<Box<dyn AudioReader>> {
    if cancel.is_cancelled() {
        return None;
    }
    config.set_cancel(cancel.child());
    config.set_host_sample_rate(rate);
    let mut resource = match Resource::new(config).await {
        Ok(r) => r,
        Err(e) => {
            warn!(?e, "analysis: resource open failed");
            return None;
        }
    };
    if let Err(e) = resource.preload().await {
        warn!(?e, "analysis: preload failed");
        return None;
    }
    Some(resource.into())
}
