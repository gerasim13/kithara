use std::{
    num::NonZeroU32,
    panic::{AssertUnwindSafe, catch_unwind},
};

use kithara_audio::AudioReader;
use kithara_platform::{
    CancelToken,
    sync::{Arc, ThreadGate, WaitGate, mpsc},
    thread::{spawn_named, yield_now},
    time::{Duration, Instant},
    tokio::sync::watch,
};
use kithara_resampler::ResamplerBackend;
use tracing::warn;

use super::{AnalysisNode, AnalysisObserver, AnalysisStep, Job};
use crate::{
    analyzer::{AnalysisFingerprint, AnalysisToken, AnalyzerBuilder, TrackAnalysis},
    producer::{AnalysisProducer, ring},
};

pub struct AnalysisWorker {
    active: bool,
    fingerprint: AnalysisFingerprint,
    job_scope: CancelToken,
    runner: AnalysisRunner,
    jobs: mpsc::Sender<Job>,
}

/// An analysis pass opened before either decoder starts.
///
/// Opening creates the bounded producer transport synchronously. The app can
/// therefore attach the producer to playback before it asynchronously opens
/// the pass's fallback reader, then hand this value back to [`AnalysisWorker::start`].
pub struct AnalysisPass {
    cancel: CancelToken,
    ingest: ring::Reader,
    rate: NonZeroU32,
    token: AnalysisToken,
    tx: watch::Sender<Option<TrackAnalysis>>,
}

impl AnalysisPass {
    /// Returns the job-scoped cancellation token for opening this pass's reader.
    #[must_use]
    pub const fn cancel_token(&self) -> &CancelToken {
        &self.cancel
    }
}

struct AnalysisRunner {
    cancel: CancelToken,
    wake: Arc<ThreadGate>,
}

impl AnalysisRunner {
    const IDLE_TIMEOUT: Duration = Duration::from_millis(10);
    const SLOW_TICK_THRESHOLD: Duration = Duration::from_millis(10);
    const YIELD_EVERY: u32 = 16;

    fn start<B>(mut node: AnalysisNode<B>, cancel: CancelToken) -> Self
    where
        B: ResamplerBackend,
    {
        let wake = Arc::new(ThreadGate::default());
        let thread_wake = Arc::clone(&wake);
        let thread_cancel = cancel.clone();
        spawn_named("kithara-analysis", move || {
            let mut observer = AnalysisObserver::default();
            let mut progress_streak = 0u32;
            loop {
                if thread_cancel.is_cancelled() {
                    node.cancel();
                    return;
                }

                let wake_edge = thread_wake.current();
                let started = Instant::now();
                let Ok(step) = catch_unwind(AssertUnwindSafe(|| node.tick())) else {
                    warn!("analysis worker node panicked");
                    node.cancel();
                    return;
                };
                let elapsed = started.elapsed();
                if elapsed > Self::SLOW_TICK_THRESHOLD {
                    AnalysisObserver::observe_slow_tick(elapsed);
                }
                observer.observe(step);

                match step {
                    AnalysisStep::Progress => {
                        progress_streak += 1;
                        if progress_streak >= Self::YIELD_EVERY {
                            progress_streak = 0;
                            yield_now();
                        }
                    }
                    AnalysisStep::Waiting | AnalysisStep::UpstreamPending => {
                        progress_streak = 0;
                        thread_wake.wait_timeout(wake_edge, Self::IDLE_TIMEOUT);
                    }
                    AnalysisStep::Done => return,
                }
            }
        });
        Self { cancel, wake }
    }

    fn wake(&self) {
        self.wake.signal();
    }

    fn shutdown(&self) {
        self.cancel.cancel();
        self.wake();
    }
}

impl AnalysisWorker {
    #[must_use]
    pub fn new<B>(parent: &CancelToken, builder: AnalyzerBuilder<B>) -> Self
    where
        B: ResamplerBackend,
    {
        let cancel = parent.child();
        let job_scope = cancel.child();
        let (jobs, receiver) = mpsc::channel();
        let node = AnalysisNode::new(builder, receiver);
        let (fingerprint, active) = node.effective();
        let runner = AnalysisRunner::start(node, cancel);
        Self {
            active,
            fingerprint,
            job_scope,
            runner,
            jobs,
        }
    }

    /// Whether detector initialization left at least one effective analyzer.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.active
    }

    /// Identity of the analyzers that survived worker initialization.
    #[must_use]
    pub const fn fingerprint(&self) -> &AnalysisFingerprint {
        &self.fingerprint
    }

    /// Open a pass and its bounded playback producer without waiting for the
    /// fallback reader to open or preload.
    #[must_use]
    pub fn open(
        &self,
        token: AnalysisToken,
        rate: NonZeroU32,
    ) -> (
        watch::Receiver<Option<TrackAnalysis>>,
        AnalysisProducer,
        AnalysisPass,
    ) {
        let (tx, rx) = watch::channel(None);
        let (writer, ingest) = ring::open_for(rate);
        let producer = AnalysisProducer::new(writer, rate, token.clone());
        let pass = AnalysisPass {
            cancel: self.job_scope.child(),
            ingest,
            rate,
            token,
            tx,
        };
        (rx, producer, pass)
    }

    /// Start an already-open pass with its fallback reader.
    pub fn start(&self, pass: AnalysisPass, reader: Box<dyn AudioReader>) {
        let AnalysisPass {
            cancel,
            ingest,
            rate,
            token,
            tx,
        } = pass;
        self.submit(Job {
            reader,
            cancel,
            ingest,
            rate,
            token,
            tx,
        });
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
    ) -> (watch::Receiver<Option<TrackAnalysis>>, AnalysisProducer) {
        let (rx, producer, pass) = self.open(token, rate);
        self.start(pass, reader);
        (rx, producer)
    }

    fn submit(&self, job: Job) {
        if self.jobs.send(job).is_err() {
            warn!("analysis worker stopped; job dropped");
        } else {
            self.runner.wake();
        }
    }
}

impl Drop for AnalysisWorker {
    fn drop(&mut self) {
        self.runner.shutdown();
    }
}

#[cfg(all(test, feature = "analysis-beat", not(feature = "beat-nn")))]
mod tests {
    use kithara_bufpool::SamplePool;
    use kithara_platform::CancelToken;
    use kithara_resampler::NoResamplerBackend;
    use kithara_test_utils::kithara;

    use super::AnalysisWorker;
    use crate::AnalyzerBuilder;

    #[kithara::test(native, flash(false))]
    fn beat_without_a_detector_is_not_an_effective_analyzer() {
        let cancel = CancelToken::never();
        let worker = AnalysisWorker::new(
            &cancel,
            AnalyzerBuilder::<NoResamplerBackend>::new(SamplePool::default()).with_beat(),
        );

        assert!(!worker.is_active());
        assert_eq!(worker.fingerprint().beat(), None);
        assert_eq!(worker.fingerprint().waveform(), None);
    }
}
