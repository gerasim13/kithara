use std::num::NonZeroU32;

use kithara_platform::{CancelToken, sync::mpsc, tokio::sync::watch};
use kithara_resampler::ResamplerBackend;
use tracing::warn;

use super::{AnalysisNode, AnalysisObserver, Job};
use crate::{
    PcmReader,
    analysis::{
        analyzer::{AnalysisToken, AnalyzerBuilder, TrackAnalysis},
        producer::{AnalysisProducer, ring},
    },
    runtime::{Scheduler, SchedulerHandle},
};

const ANALYSIS_NODE_ID: u64 = 1;

pub struct AnalysisWorker<B>
where
    B: ResamplerBackend,
{
    job_scope: JobScope,
    scheduler: SchedulerHandle<AnalysisNode<B>>,
    jobs: mpsc::Sender<Job>,
}

struct JobScope(CancelToken);

impl JobScope {
    fn child(&self) -> CancelToken {
        self.0.child()
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
        let node = AnalysisNode::new(builder, receiver);
        let scheduler = Scheduler::<AnalysisNode<B>, AnalysisObserver>::start(
            "kithara-analysis".into(),
            AnalysisObserver::default(),
            cancel,
        );
        scheduler.register(ANALYSIS_NODE_ID, node);
        Self {
            job_scope,
            scheduler,
            jobs,
        }
    }

    /// Open a pass on `rate`, the axis its ranges are measured on; a chunk on
    /// another axis is refused. Returns where its snapshots arrive and the
    /// producer another component may contribute decoded ranges through.
    pub fn analyze(
        &self,
        reader: Box<dyn PcmReader>,
        cancel: CancelToken,
        token: AnalysisToken,
        rate: NonZeroU32,
    ) -> (watch::Receiver<Option<TrackAnalysis>>, AnalysisProducer) {
        let (tx, rx) = watch::channel(None);
        let (writer, ingest) = ring::open_for(rate);
        let producer = AnalysisProducer::new(writer, rate, token.clone());
        if self
            .jobs
            .send(Job {
                reader,
                cancel,
                ingest,
                rate,
                token,
                tx,
            })
            .is_err()
        {
            warn!("analysis worker stopped; job dropped");
        } else {
            self.scheduler.wake();
        }
        (rx, producer)
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
        self.scheduler.shutdown();
    }
}
