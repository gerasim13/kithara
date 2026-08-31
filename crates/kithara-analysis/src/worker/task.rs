use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};

use kithara_audio::{AudioReader, ChunkOutcome, SeekOutcome};
use kithara_bufpool::SampleBuffer;
use kithara_platform::{CancelToken, tokio::sync::watch};
use kithara_resampler::ResamplerBackend;
use kithara_signal::AudioSpec;
use kithara_worker::TickResult;
use tracing::{debug, warn};

use super::schedule::{Extent, Schedule};
use crate::{
    AnalysisProgress, BlobError,
    analyzer::{AnalysisToken, AnalyzerBuilder, Detector, Ingest, TrackAnalyzers},
    coverage::{Coverage, FrameRange},
    producer::ring,
    slots::beat::{DetectionOutput, DetectionRequest},
};

pub(crate) struct Job {
    pub(crate) reader: Box<dyn AudioReader>,
    pub(crate) cancel: CancelToken,
    pub(crate) ingest: ring::Reader,
    pub(crate) rate: NonZeroU32,
    pub(crate) token: AnalysisToken,
    pub(crate) tx: watch::Sender<Option<AnalysisProgress>>,
    pub(crate) resume: Option<AnalysisProgress>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TaskPhase {
    Decode,
    Ending,
    Done,
}

struct Run {
    chosen: u64,
    at: u64,
    frontier: u64,
    started: bool,
    grew: bool,
}

#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub(crate) struct AnalysisTask<B>
where
    B: ResamplerBackend,
{
    reader: Box<dyn AudioReader>,
    #[field(get = cancel_token, vis = "pub(crate)")]
    cancel: CancelToken,
    analyzers: Option<TrackAnalyzers<B>>,
    ingest: ring::Reader,
    scratch: Option<SampleBuffer>,
    rate: NonZeroU32,
    token: AnalysisToken,
    tx: watch::Sender<Option<AnalysisProgress>>,
    phase: TaskPhase,
    beat_dirty: bool,
    chunk_frames: NonZeroU64,
    producer_drain_limit: usize,
    publish_frames: u64,
    published_at: u64,
    extent: Extent,
    schedule: Schedule,
    run: Option<Run>,
}

impl<B> AnalysisTask<B>
where
    B: ResamplerBackend,
{
    pub(crate) fn new(
        job: Job,
        builder: &AnalyzerBuilder<B>,
        chunk_seconds: NonZeroU32,
        producer_drain_limit: NonZeroUsize,
        publish_seconds: NonZeroU32,
    ) -> Result<Self, BlobError> {
        let rate = u64::from(job.rate.get());
        let chunk_frames = NonZeroU64::new(rate.saturating_mul(u64::from(chunk_seconds.get())))
            .ok_or(BlobError::Corrupt)?;
        let analyzers = job
            .resume
            .as_ref()
            .map(|progress| builder.restore(progress, chunk_frames))
            .transpose()?;
        let published_at = analyzers.as_ref().map_or(0, TrackAnalyzers::covered_frames);
        let extent = job
            .resume
            .as_ref()
            .and_then(|progress| progress.analysis().extent())
            .map_or_else(Extent::default, Extent::restore);
        Ok(Self {
            analyzers,
            beat_dirty: false,
            cancel: job.cancel,
            chunk_frames,
            extent,
            ingest: job.ingest,
            phase: TaskPhase::Decode,
            producer_drain_limit: producer_drain_limit.get(),
            publish_frames: rate.saturating_mul(u64::from(publish_seconds.get())),
            published_at,
            rate: job.rate,
            reader: job.reader,
            run: None,
            schedule: Schedule::default(),
            scratch: None,
            token: job.token,
            tx: job.tx,
        })
    }

    fn is_covered(&self, range: FrameRange) -> bool {
        self.analyzers
            .as_ref()
            .is_some_and(|analyzers| analyzers.coverage().contains(range))
    }

    fn is_complete(&self) -> bool {
        self.extent
            .frames()
            .is_some_and(|extent| self.is_covered(FrameRange::new(0, extent)))
    }

    fn choose(&self, window: Option<u64>) -> Option<u64> {
        let empty = Coverage::default();
        let coverage = self
            .analyzers
            .as_ref()
            .map_or(&empty, TrackAnalyzers::coverage);
        self.schedule.next(coverage, self.extent.frames(), window)
    }

    fn decode(
        &mut self,
        builder: &AnalyzerBuilder<B>,
        detector: Option<&mut Detector>,
    ) -> TickResult {
        match self.reader.next_chunk() {
            Ok(ChunkOutcome::Chunk(chunk)) => {
                let range = FrameRange::from(&chunk.meta);
                let analyzers = open(&mut self.analyzers, builder, self.rate, &self.token);
                let before = analyzers.covered_frames();
                let outcome = analyzers.push(&chunk, detector);
                if outcome != Ingest::Accepted {
                    debug!(?outcome, "analysis: range not folded in");
                }
                let grew = analyzers.covered_frames() > before;
                if let Some(run) = &mut self.run {
                    if !run.started {
                        run.started = true;
                        run.at = range.start();
                    }
                    run.frontier = range.end();
                    run.grew |= grew;
                }
                TickResult::Progress
            }
            Ok(ChunkOutcome::Pending { .. }) => TickResult::UpstreamPending,
            Ok(ChunkOutcome::Eof { .. }) => {
                // End of stream bounds the extent; gaps behind it may remain.
                if self.extent.frames().is_none() {
                    self.finish(true);
                } else {
                    if let Some(run) = &self.run {
                        self.extent.unreachable(run.frontier);
                        debug!(
                            frontier = run.frontier,
                            extent = ?self.extent.frames(),
                            "analysis: eof bounds the extent"
                        );
                    }
                    self.retire();
                }
                TickResult::Progress
            }
            Err(error) => {
                // The reader failed; the ranges it delivered did not.
                warn!(?error, "analysis: decode error; pass ended");
                self.finish(false);
                TickResult::Progress
            }
        }
    }

    fn drain(&mut self, builder: &AnalyzerBuilder<B>, detector: Option<&mut Detector>) -> bool {
        let scratch = self
            .scratch
            .get_or_insert_with(|| builder.sample_pool().get_with(Vec::clear));
        let analyzers = open(&mut self.analyzers, builder, self.rate, &self.token);
        let mut detector = detector;
        let mut folded = false;

        for _ in 0..self.producer_drain_limit {
            let Some(at) = self.ingest.pop(scratch) else {
                break;
            };
            let outcome = analyzers.push_mono(scratch, at, detector.as_deref_mut());
            if outcome != Ingest::Accepted {
                debug!(?outcome, at, "analysis: offered range not folded in");
            }
            folded = true;
        }
        folded
    }

    fn due(&self) -> bool {
        let Some(analyzers) = &self.analyzers else {
            return false;
        };
        analyzers.covered_frames().saturating_sub(self.published_at) >= self.publish_frames
    }

    fn sync_extent(&mut self) {
        let Some(frames) = self.extent.frames() else {
            return;
        };
        if let Some(analyzers) = &mut self.analyzers {
            analyzers.plan_extent(frames);
        }
    }

    /// `settled` when the pass ran out of reachable ranges rather than being
    /// cut short: that is what tells a consumer this is all the content gives.
    fn finish(&mut self, settled: bool) {
        let planned = self.extent.frames();
        debug!(
            extent = ?planned,
            covered = ?self.analyzers.as_ref().map(TrackAnalyzers::covered_frames),
            settled,
            "analysis: pass ended"
        );
        let Some(analyzers) = &mut self.analyzers else {
            self.phase = TaskPhase::Done;
            return;
        };
        if let Some(frames) = planned {
            analyzers.plan_extent(frames);
        }
        if settled {
            analyzers.settle();
        }
        self.phase = TaskPhase::Ending;
    }

    pub(crate) fn is_done(&self) -> bool {
        self.phase == TaskPhase::Done
    }

    pub(crate) fn is_ending(&self) -> bool {
        self.phase == TaskPhase::Ending
    }

    pub(crate) fn prepare_detection(&mut self) -> Option<DetectionRequest> {
        let trailing = self.is_ending();
        self.analyzers.as_mut()?.prepare_detection(trailing)
    }

    pub(crate) fn apply_detection(&mut self, output: DetectionOutput) {
        if let Some(analyzers) = &mut self.analyzers {
            analyzers.apply_detection(output);
            self.beat_dirty = true;
        }
    }

    pub(crate) fn fail_compute_unavailable(&mut self) {
        warn!("analysis: compute pool unavailable; pass ended");
        self.phase = TaskPhase::Done;
    }

    fn publish(&mut self, detector: Option<&mut Detector>, ending: bool) {
        let Some(analyzers) = &mut self.analyzers else {
            return;
        };
        if analyzers.covered_frames() == 0 {
            return;
        }
        let progress = analyzers.progress(detector, ending, self.chunk_frames);
        self.published_at = analyzers.covered_frames();
        self.tx.send(Some(progress)).ok();
    }

    fn reschedule(&mut self, window: Option<u64>) -> TickResult {
        self.retire();
        let Some(at) = self.choose(window) else {
            self.finish(true);
            return TickResult::Progress;
        };

        let Ok(position) = AudioSpec::new(1, self.rate).duration_for(at) else {
            warn!(
                at,
                "analysis: scheduled frame cannot be represented as a duration"
            );
            self.schedule.barren(at);
            return TickResult::Progress;
        };
        match self.reader.seek(position) {
            // `landed_at` only echoes the target here; the first chunk says where.
            Ok(SeekOutcome::Landed { .. }) => {
                debug!(at, "analysis: run scheduled");
                self.run = Some(Run {
                    chosen: at,
                    at,
                    frontier: at,
                    started: false,
                    grew: false,
                });
            }
            // The source cannot deliver the position the schedule planned
            // against, which bounds where it ends however long it says it is.
            Ok(SeekOutcome::PastEof { duration, .. }) => {
                debug!(at, ?duration, "analysis: scheduled position past the end");
                self.extent.unreachable(at);
            }
            Err(error) => {
                warn!(?error, at, "analysis: seek failed; position retired");
                self.schedule.barren(at);
            }
        }
        TickResult::Progress
    }

    fn retire(&mut self) {
        let Some(run) = self.run.take() else {
            return;
        };
        // What the run itself decoded, not what the pass covered while it ran:
        // a producer folds ranges from anywhere and would keep this alive.
        if !run.grew {
            debug!(at = run.chosen, "analysis: position added nothing; retired");
            self.schedule.barren(run.chosen);
        }
    }

    fn run_over(&self, run_frames: Option<u64>) -> bool {
        let Some(run) = &self.run else {
            return true;
        };
        if self
            .extent
            .frames()
            .is_some_and(|extent| run.frontier >= extent)
        {
            return true;
        }
        // Covered audio ends a run that already reached its gap. Before that
        // it is the lead-in a seek snapping back off the gap's start left in
        // front, and ending there would retire the gap unread.
        if run.grew && self.is_covered(FrameRange::new(run.frontier, 1)) {
            return true;
        }
        // Read past what it was aimed at with nothing gained: the gap the
        // schedule saw there is not where this source can put the reader.
        if !run.grew && run.frontier > run.chosen {
            return true;
        }
        run_frames.is_some_and(|window| run.frontier.saturating_sub(run.at) >= window)
    }

    fn step(
        &mut self,
        builder: &AnalyzerBuilder<B>,
        detector: Option<&mut Detector>,
    ) -> TickResult {
        if self.extent.frames().is_none() {
            return self.decode(builder, detector);
        }
        if self.is_complete() {
            self.finish(true);
            return TickResult::Progress;
        }
        let window = Some(self.chunk_frames.get());
        if self.run_over(window) {
            return self.reschedule(window);
        }
        self.decode(builder, detector)
    }

    pub(crate) fn tick(
        &mut self,
        builder: &AnalyzerBuilder<B>,
        detector: Option<&mut Detector>,
    ) -> TickResult {
        if self.cancel.is_cancelled() && self.phase == TaskPhase::Decode {
            debug!("analysis cancelled");
            self.finish(false);
            return TickResult::Progress;
        }

        match self.phase {
            TaskPhase::Decode => {
                let mut detector = detector;
                let drained = self.drain(builder, detector.as_deref_mut());
                // Re-read: the decode path refines a duration upward as it goes.
                self.extent.report(self.reader.duration(), self.rate);
                let result = self.step(builder, detector.as_deref_mut());
                self.sync_extent();
                if self.phase == TaskPhase::Decode && (self.due() || self.beat_dirty) {
                    self.publish(detector, false);
                    self.beat_dirty = false;
                }
                if drained {
                    TickResult::Progress
                } else {
                    result
                }
            }
            TaskPhase::Ending => {
                self.publish(detector, true);
                self.phase = TaskPhase::Done;
                TickResult::Progress
            }
            TaskPhase::Done => TickResult::Done,
        }
    }
}

fn open<'a, B>(
    slot: &'a mut Option<TrackAnalyzers<B>>,
    builder: &AnalyzerBuilder<B>,
    rate: NonZeroU32,
    token: &AnalysisToken,
) -> &'a mut TrackAnalyzers<B>
where
    B: ResamplerBackend,
{
    slot.get_or_insert_with(|| builder.build(rate, token.clone()))
}
