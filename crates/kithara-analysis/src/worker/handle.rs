use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};

use kithara_audio::AudioReader;
use kithara_bufpool::HasPool;
use kithara_platform::{CancelGroup, CancelToken, sync::mpsc, tokio::sync::watch};
use kithara_resampler::ResamplerBackend;
use kithara_worker::{
    Dispatcher, DispatcherConfig, RayonConfig, TaskConfig, TaskError, TaskHandle, Worker,
    WorkerConfig,
};
use tracing::warn;

use super::{AnalysisNode, AnalysisObserver, AnalysisWorkerConfig, Job};
use crate::{
    AnalysisFileError, AnalysisProgress,
    analyzer::{AnalysisFingerprint, AnalysisToken},
    producer::{AnalysisProducer, ring},
    worker::schedule::extent_frames,
};

pub struct AnalysisWorker {
    resume_shape: (bool, bool),
    fingerprint: AnalysisFingerprint,
    job_scope: CancelToken,
    dispatcher: Dispatcher,
    chunk_seconds: NonZeroU32,
    jobs: mpsc::Sender<Job>,
    task: TaskHandle,
    _base: Worker,
    active: bool,
}

/// An analysis pass opened before either decoder starts.
///
/// Opening creates the bounded producer transport synchronously. The app can
/// therefore attach the producer to playback before it asynchronously opens
/// the pass's fallback reader, then hand this value back to [`AnalysisWorker::start`].
pub struct AnalysisPass {
    token: AnalysisToken,
    cancel: CancelToken,
    rate: NonZeroU32,
    resume: Option<AnalysisProgress>,
    ingest: ring::Reader,
    tx: watch::Sender<Option<AnalysisProgress>>,
}

/// Output of opening an analysis pass before its fallback reader starts.
pub type AnalysisOpen = (
    watch::Receiver<Option<AnalysisProgress>>,
    AnalysisProducer,
    AnalysisPass,
);

impl AnalysisPass {
    /// Returns the job-scoped cancellation token for opening this pass's reader.
    #[must_use]
    pub const fn cancel_token(&self) -> &CancelToken {
        &self.cancel
    }
}

impl AnalysisWorker {
    /// Construct the analysis dispatcher and its long-lived task.
    ///
    /// # Errors
    ///
    /// Returns the base worker's canonical task admission error when the task
    /// cannot be registered.
    pub fn new<B, S>(config: AnalysisWorkerConfig<B, S>) -> Result<Self, TaskError>
    where
        B: ResamplerBackend,
        S: HasPool<f32> + Send + Sync + 'static,
    {
        let AnalysisWorkerConfig {
            builder,
            cancel,
            capacity,
            chunk_seconds,
            fairness_yield_interval,
            idle_timeout,
            max_compute_tasks,
            priority,
            producer_drain_limit,
            publish_seconds,
            slow_tick_threshold,
            task_burst,
            wait_timeout,
            worker,
        } = config;
        let (base, dispatcher_cancel) = if let Some(worker) = worker {
            (worker, cancel.map(CancelGroup::from))
        } else {
            let worker_config = cancel
                .map_or_else(WorkerConfig::new, |cancel| {
                    WorkerConfig::new().with_cancel(cancel)
                })
                .with_max_compute_tasks(max_compute_tasks)
                .with_owned_pool(RayonConfig::new(
                    NonZeroUsize::MIN,
                    "kithara-analysis-compute",
                ));
            (Worker::new(worker_config), None)
        };
        let mut dispatcher_config = DispatcherConfig::new("kithara-analysis")
            .with_capacity(capacity)
            .with_fairness_yield_interval(fairness_yield_interval)
            .with_idle_timeout(idle_timeout)
            .with_observer(AnalysisObserver::default())
            .with_slow_tick_threshold(slow_tick_threshold)
            .with_task_burst(task_burst)
            .with_wait_timeout(wait_timeout);
        if let Some(cancel) = dispatcher_cancel {
            dispatcher_config = dispatcher_config.with_cancel(cancel);
        }
        let dispatcher = base.dispatcher(dispatcher_config);
        let pending = dispatcher.reserve(
            TaskConfig::new()
                .with_max_compute_tasks(max_compute_tasks)
                .with_priority(priority),
        )?;
        let job_scope = pending.context().token().clone();
        let context = pending.context().clone();
        let (jobs, receiver) = mpsc::channel();
        let node = AnalysisNode::new(
            builder,
            receiver,
            context,
            chunk_seconds,
            producer_drain_limit,
            publish_seconds,
        );
        let (fingerprint, active, resume_shape) = node.effective();
        let task = pending.start(|_| node)?;

        Ok(Self {
            active,
            chunk_seconds,
            dispatcher,
            fingerprint,
            job_scope,
            jobs,
            resume_shape,
            task,
            _base: base,
        })
    }

    /// Open a pass on `rate`, the axis its ranges are measured on; a chunk on
    /// another axis is refused. Returns where its snapshots arrive and the
    /// producer another component may contribute decoded ranges through.
    #[must_use]
    pub fn analyze(
        &self,
        reader: Box<dyn AudioReader>,
        token: AnalysisToken,
        rate: NonZeroU32,
    ) -> (watch::Receiver<Option<AnalysisProgress>>, AnalysisProducer) {
        let (rx, producer, pass) = self.open(token, rate);
        self.start(pass, reader);
        (rx, producer)
    }

    /// Identity of the analyzers that survived worker initialization.
    #[must_use]
    pub const fn fingerprint(&self) -> &AnalysisFingerprint {
        &self.fingerprint
    }

    /// Whether detector initialization left at least one effective analyzer.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.active
    }

    /// Open a pass and its bounded playback producer without waiting for the
    /// fallback reader to open or preload.
    #[must_use]
    pub fn open(
        &self,
        token: AnalysisToken,
        rate: NonZeroU32,
    ) -> (
        watch::Receiver<Option<AnalysisProgress>>,
        AnalysisProducer,
        AnalysisPass,
    ) {
        let (tx, rx) = watch::channel(None);
        let (writer, ingest) = ring::open_for(rate);
        let producer = AnalysisProducer::new(writer, rate, token.clone());
        let pass = AnalysisPass {
            ingest,
            rate,
            token,
            tx,
            cancel: self.job_scope.child(),
            resume: None,
        };
        (rx, producer, pass)
    }

    /// Open a validated partial publication before its fallback reader is
    /// available, returning the producer synchronously for playback ingress.
    ///
    /// # Errors
    ///
    /// Rejects a settled or malformed checkpoint, analyzer/config drift, an
    /// unknown source extent, and a different configured chunk size.
    pub fn open_resume(
        &self,
        progress: AnalysisProgress,
    ) -> Result<AnalysisOpen, AnalysisFileError> {
        progress.validate_resume()?;
        let analysis = progress.analysis();
        let rate = analysis.source_sample_rate();
        let extent = analysis.extent().ok_or(AnalysisFileError::UnknownExtent)?;
        let (chunk_frames, shape) = progress.resume_meta().ok_or(AnalysisFileError::Config)?;
        let expected_chunk = NonZeroU64::new(
            u64::from(rate.get()).saturating_mul(u64::from(self.chunk_seconds.get())),
        )
        .ok_or(AnalysisFileError::Config)?;
        if analysis.is_settled()
            || analysis.fingerprint() != &self.fingerprint
            || chunk_frames != expected_chunk
            || shape != self.resume_shape
            || analysis
                .coverage()
                .runs()
                .iter()
                .any(|range| range.end() > extent)
        {
            return Err(AnalysisFileError::Config);
        }

        let token = analysis.token().clone();
        let (tx, rx) = watch::channel(Some(progress.clone()));
        let (writer, ingest) = ring::open_for(rate);
        let producer = AnalysisProducer::new(writer, rate, token.clone());
        let pass = AnalysisPass {
            ingest,
            rate,
            token,
            tx,
            cancel: self.job_scope.child(),
            resume: Some(progress),
        };
        Ok((rx, producer, pass))
    }

    /// Start an already-open pass with its fallback reader.
    pub fn start(&self, pass: AnalysisPass, reader: Box<dyn AudioReader>) {
        if pass.resume.is_some() {
            warn!("analysis resume pass requires extent validation");
            return;
        }
        self.submit_pass(pass, reader);
    }

    /// Start a resume pass only after its opened reader confirms the persisted
    /// source extent.
    ///
    /// # Errors
    ///
    /// Rejects a fresh pass, an unknown reader duration, or an extent that no
    /// longer matches the validated checkpoint.
    pub fn start_resume(
        &self,
        pass: AnalysisPass,
        reader: Box<dyn AudioReader>,
    ) -> Result<(), AnalysisFileError> {
        let progress = pass.resume.as_ref().ok_or(AnalysisFileError::Config)?;
        let extent = progress
            .analysis()
            .extent()
            .ok_or(AnalysisFileError::UnknownExtent)?;
        if extent_frames(reader.duration(), pass.rate) != Some(extent) {
            return Err(AnalysisFileError::Config);
        }
        self.submit_pass(pass, reader);
        Ok(())
    }

    fn submit(&self, job: Job) {
        if self.jobs.send(job).is_err() {
            warn!("analysis worker stopped; job dropped");
        } else {
            self.task.control().wake();
        }
    }

    fn submit_pass(&self, pass: AnalysisPass, reader: Box<dyn AudioReader>) {
        let AnalysisPass {
            cancel,
            ingest,
            rate,
            token,
            tx,
            resume,
        } = pass;
        self.submit(Job {
            reader,
            cancel,
            ingest,
            rate,
            token,
            tx,
            resume,
        });
    }
}

impl Drop for AnalysisWorker {
    fn drop(&mut self) {
        self.dispatcher.shutdown();
    }
}

#[cfg(all(test, feature = "analysis-beat", not(feature = "beat-nn")))]
mod tests {
    use kithara_platform::CancelToken;
    use kithara_resampler::NoResamplerBackend;
    use kithara_test_utils::kithara;

    use super::{AnalysisWorker, AnalysisWorkerConfig};
    use crate::{AnalyzerBuilder, test_pools::pools};

    #[kithara::test(native, flash(false))]
    fn beat_without_a_detector_is_not_an_effective_analyzer() {
        let cancel = CancelToken::never();
        let worker = AnalysisWorker::new(
            AnalysisWorkerConfig::for_builder(
                AnalyzerBuilder::<NoResamplerBackend, _>::new(pools()).with_beat(),
            )
            .cancel(cancel)
            .build(),
        )
        .expect("analysis worker task is admitted");

        assert!(!worker.is_active());
        assert_eq!(worker.fingerprint().beat(), None);
        assert_eq!(worker.fingerprint().waveform(), None);
    }
}
