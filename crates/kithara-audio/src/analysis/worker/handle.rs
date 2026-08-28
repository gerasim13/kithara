use std::panic::{AssertUnwindSafe, catch_unwind};

use kithara_platform::{
    CancelToken,
    sync::{Arc, mpsc},
    thread::{spawn_named, yield_now},
    time::{Duration, Instant},
    tokio::sync::watch,
};
use kithara_resampler::ResamplerBackend;
use tracing::warn;

use super::{AnalysisNode, AnalysisObserver, AnalysisStep, Job};
use crate::{
    AudioReader,
    analysis::analyzer::{AnalyzerBuilder, TrackAnalysis},
    runtime::{WakeSignal, wake::ThreadWake},
};

pub struct AnalysisWorker<B>
where
    B: ResamplerBackend,
{
    job_scope: JobScope,
    runner: AnalysisRunner,
    jobs: mpsc::Sender<Job>,
    _backend: std::marker::PhantomData<B>,
}

struct JobScope(CancelToken);

impl JobScope {
    fn child(&self) -> CancelToken {
        self.0.child()
    }
}

struct AnalysisRunner {
    cancel: CancelToken,
    wake: Arc<ThreadWake>,
}

impl AnalysisRunner {
    const IDLE_TIMEOUT: Duration = Duration::from_millis(10);
    const SLOW_TICK_THRESHOLD: Duration = Duration::from_millis(10);
    const YIELD_EVERY: u32 = 16;

    fn start<B>(mut node: AnalysisNode<B>, cancel: CancelToken) -> Self
    where
        B: ResamplerBackend,
    {
        let wake = Arc::new(ThreadWake::default());
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
        self.wake.wake();
    }

    fn shutdown(&self) {
        self.cancel.cancel();
        self.wake();
    }
}

impl<B> AnalysisWorker<B>
where
    B: ResamplerBackend,
{
    #[must_use]
    pub fn new(parent: &CancelToken, builder: AnalyzerBuilder<B>) -> Self {
        let cancel = parent.child();
        let job_scope = JobScope(cancel.child());
        let (jobs, receiver) = mpsc::channel();
        let runner = AnalysisRunner::start(AnalysisNode::new(builder, receiver), cancel);
        Self {
            job_scope,
            runner,
            jobs,
            _backend: std::marker::PhantomData,
        }
    }

    pub fn analyze(
        &self,
        reader: Box<dyn AudioReader>,
        cancel: CancelToken,
    ) -> watch::Receiver<Option<TrackAnalysis>> {
        let (tx, rx) = watch::channel(None);
        if self.jobs.send(Job { reader, cancel, tx }).is_err() {
            warn!("analysis worker stopped; job dropped");
        } else {
            self.runner.wake();
        }
        rx
    }

    #[must_use]
    pub fn child_token(&self) -> CancelToken {
        self.job_scope.child()
    }
}

impl<B> Drop for AnalysisWorker<B>
where
    B: ResamplerBackend,
{
    fn drop(&mut self) {
        self.runner.shutdown();
    }
}
