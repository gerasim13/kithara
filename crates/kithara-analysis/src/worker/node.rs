use std::num::{NonZeroU32, NonZeroUsize};

use kithara_platform::{
    CancelToken,
    sync::mpsc::{self, Receiver, Sender, TryRecvError},
};
use kithara_resampler::ResamplerBackend;
use kithara_worker::{ComputeSubmitError, Task, TaskContext, TickResult};
use tracing::warn;

use super::AnalysisTask;
pub(crate) use super::task::Job;
use crate::{
    analyzer::{AnalysisFingerprint, AnalyzerBuilder, Detector},
    slots::beat::{DetectionOutput, DetectionRequest, detect},
};

struct DetectJob {
    pass_cancel: CancelToken,
    detector: Detector,
    request: DetectionRequest,
}

struct DetectDone {
    detector: Detector,
    output: Option<DetectionOutput>,
}

enum DetectorState {
    Disabled,
    Idle(Detector),
    Retry(DetectJob),
    Running,
    Unavailable,
}

pub(crate) struct AnalysisNode<B>
where
    B: ResamplerBackend,
{
    builder: AnalyzerBuilder<B>,
    chunk_seconds: NonZeroU32,
    completed: Receiver<DetectDone>,
    completion: Sender<DetectDone>,
    context: TaskContext,
    current: Option<AnalysisTask<B>>,
    detector: DetectorState,
    jobs: Receiver<Job>,
    producer_drain_limit: NonZeroUsize,
    publish_seconds: NonZeroU32,
}

impl<B> AnalysisNode<B>
where
    B: ResamplerBackend,
{
    pub(crate) fn new(
        mut builder: AnalyzerBuilder<B>,
        jobs: Receiver<Job>,
        context: TaskContext,
        chunk_seconds: NonZeroU32,
        producer_drain_limit: NonZeroUsize,
        publish_seconds: NonZeroU32,
    ) -> Self {
        let detector = builder
            .take_detector()
            .map_or(DetectorState::Disabled, DetectorState::Idle);
        let (completion, completed) = mpsc::channel();
        Self {
            builder,
            chunk_seconds,
            completed,
            completion,
            context,
            current: None,
            detector,
            jobs,
            producer_drain_limit,
            publish_seconds,
        }
    }

    pub(crate) fn effective(&self) -> (AnalysisFingerprint, bool, (bool, bool)) {
        (
            self.builder.fingerprint(),
            !self.builder.is_empty(),
            self.builder.resume_shape(),
        )
    }

    fn accept_completion(&mut self) -> bool {
        let done = match self.completed.try_recv() {
            Ok(done) => done,
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => return false,
        };
        self.detector = DetectorState::Idle(done.detector);
        let Some(current) = &mut self.current else {
            return true;
        };
        if !current.cancel_token().is_cancelled()
            && let Some(output) = done.output
        {
            current.apply_detection(output);
        }
        true
    }

    fn submit_detection(&mut self, job: DetectJob) -> TickResult {
        let completion = self.completion.clone();
        match self.context.submit_compute(job, move |compute, mut job| {
            let cancel = compute.cancel_group().clone() | job.pass_cancel.clone();
            let output = if cancel.is_cancelled() {
                None
            } else {
                let output = detect(job.request, &mut job.detector);
                (!cancel.is_cancelled()).then_some(output)
            };
            completion
                .send(DetectDone {
                    detector: job.detector,
                    output,
                })
                .ok();
        }) {
            Ok(()) => {
                self.detector = DetectorState::Running;
                TickResult::Progress
            }
            Err(rejected) if rejected.reason() == ComputeSubmitError::Saturated => {
                self.detector = DetectorState::Retry(rejected.recover_payload());
                TickResult::Backpressured
            }
            Err(rejected) => {
                let reason = rejected.reason();
                drop(rejected.recover_payload());
                self.detector = DetectorState::Unavailable;
                if let Some(current) = &mut self.current {
                    current.fail_compute_unavailable();
                }
                debug_assert!(matches!(
                    reason,
                    ComputeSubmitError::Cancelled | ComputeSubmitError::Unavailable
                ));
                TickResult::Progress
            }
        }
    }

    fn drive_detection(&mut self) -> Option<TickResult> {
        let state = std::mem::replace(&mut self.detector, DetectorState::Disabled);
        match state {
            DetectorState::Disabled => {
                self.detector = DetectorState::Disabled;
                None
            }
            DetectorState::Unavailable => {
                self.detector = DetectorState::Unavailable;
                let current = self.current.as_mut()?;
                if current.prepare_detection().is_some() {
                    current.fail_compute_unavailable();
                    Some(TickResult::Progress)
                } else {
                    None
                }
            }
            DetectorState::Running => {
                self.detector = DetectorState::Running;
                self.current
                    .as_ref()
                    .is_some_and(AnalysisTask::is_ending)
                    .then_some(TickResult::Backpressured)
            }
            DetectorState::Retry(job) => {
                let live = self.current.is_some() && !job.pass_cancel.is_cancelled();
                if live {
                    Some(self.submit_detection(job))
                } else {
                    self.detector = DetectorState::Idle(job.detector);
                    None
                }
            }
            DetectorState::Idle(detector) => {
                let Some(current) = &mut self.current else {
                    self.detector = DetectorState::Idle(detector);
                    return None;
                };
                if current.cancel_token().is_cancelled() {
                    self.detector = DetectorState::Idle(detector);
                    return None;
                }
                let Some(request) = current.prepare_detection() else {
                    self.detector = DetectorState::Idle(detector);
                    return None;
                };
                let pass_cancel = current.cancel_token().clone();
                Some(self.submit_detection(DetectJob {
                    pass_cancel,
                    detector,
                    request,
                }))
            }
        }
    }

    fn clear_finished(&mut self) {
        if !self.current.as_ref().is_some_and(AnalysisTask::is_done) {
            return;
        }
        self.current = None;
        let state = std::mem::replace(&mut self.detector, DetectorState::Disabled);
        self.detector = match state {
            DetectorState::Retry(job) => DetectorState::Idle(job.detector),
            state => state,
        };
    }

    fn tick_current(&mut self) -> TickResult {
        let ending = self.current.as_ref().is_some_and(AnalysisTask::is_ending);
        let cancelled = self
            .current
            .as_ref()
            .is_some_and(|current| current.cancel_token().is_cancelled());
        if ending
            && !cancelled
            && let Some(result) = self.drive_detection()
        {
            return result;
        }

        let Some(current) = &mut self.current else {
            return TickResult::UpstreamPending;
        };
        let result = current.tick(&self.builder, None);
        self.clear_finished();
        if self.current.is_none() {
            return TickResult::Progress;
        }
        match self.drive_detection() {
            Some(TickResult::Progress) => TickResult::Progress,
            Some(waiting) if result != TickResult::Progress => waiting,
            _ => result,
        }
    }

    fn accept_job(&mut self) -> TickResult {
        match self.jobs.try_recv() {
            Ok(job) => {
                let task = match AnalysisTask::new(
                    job,
                    &self.builder,
                    self.chunk_seconds,
                    self.producer_drain_limit,
                    self.publish_seconds,
                ) {
                    Ok(task) => task,
                    Err(error) => {
                        warn!(?error, "analysis: resume checkpoint rejected");
                        return TickResult::Progress;
                    }
                };
                self.current = Some(task);
                self.tick_current()
            }
            Err(TryRecvError::Empty) => TickResult::UpstreamPending,
            Err(TryRecvError::Disconnected) => TickResult::Done,
        }
    }
}

impl<B> Task for AnalysisNode<B>
where
    B: ResamplerBackend,
{
    fn tick(&mut self) -> TickResult {
        let completed = self.accept_completion();
        let result = if self.current.is_some() {
            self.tick_current()
        } else {
            self.accept_job()
        };
        if completed && result != TickResult::Done {
            TickResult::Progress
        } else {
            result
        }
    }

    fn on_cancel(&mut self) {
        self.current = None;
    }
}
