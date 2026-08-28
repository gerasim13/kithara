use kithara_platform::sync::mpsc::{Receiver, TryRecvError};
use kithara_resampler::ResamplerBackend;

pub(crate) use super::task::Job;
use super::{AnalysisStep, AnalysisTask};
use crate::analysis::analyzer::{AnalysisFingerprint, AnalyzerBuilder, Detector};

pub(crate) struct AnalysisNode<B>
where
    B: ResamplerBackend,
{
    builder: AnalyzerBuilder<B>,
    current: Option<AnalysisTask<B>>,
    detector: Option<Detector>,
    jobs: Receiver<Job>,
}

impl<B> AnalysisNode<B>
where
    B: ResamplerBackend,
{
    pub(crate) fn new(mut builder: AnalyzerBuilder<B>, jobs: Receiver<Job>) -> Self {
        let detector = builder.take_detector();
        Self {
            builder,
            detector,
            jobs,
            current: None,
        }
    }

    pub(crate) fn effective(&self) -> (AnalysisFingerprint, bool) {
        (self.builder.fingerprint(), !self.builder.is_empty())
    }

    fn tick_current(&mut self) -> AnalysisStep {
        let Some(current) = &mut self.current else {
            return AnalysisStep::UpstreamPending;
        };
        let result = current.tick(&self.builder, self.detector.as_mut());
        if current.is_done() {
            self.current = None;
        }
        result
    }
}

impl<B> AnalysisNode<B>
where
    B: ResamplerBackend,
{
    pub(crate) fn cancel(&mut self) {
        self.current = None;
    }

    pub(crate) fn tick(&mut self) -> AnalysisStep {
        if self.current.is_some() {
            return self.tick_current();
        }

        match self.jobs.try_recv() {
            Ok(job) => {
                self.current = Some(AnalysisTask::new(job));
                self.tick_current()
            }
            Err(TryRecvError::Empty) => AnalysisStep::UpstreamPending,
            Err(TryRecvError::Disconnected) => AnalysisStep::Done,
        }
    }
}
