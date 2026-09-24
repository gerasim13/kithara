use std::num::NonZeroU64;

use kithara_platform::tokio::sync::oneshot;
use kithara_signal::AudioChunkInfo;
use kithara_warp::WarpPlan;
use kithara_worker::PendingTask;

/// Verdict a staged lane owes the preparation it was opened for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Readiness {
    /// The lane's ring holds mapped PCM from the plan's activation on, as
    /// much as the playback budget needs.
    Ready,
    /// The lane ended, failed, or produced audio that cannot present the
    /// plan's activation.
    Failed,
}

/// Watches the PCM a staged lane admits to its ring until it proves, or
/// disproves, that the lane holds the entered plan from its activation.
///
/// Only admitted chunks count: a preload latch or a seek outcome says
/// nothing about the plan. The verdict is published once, outside the
/// producer step; a cancelled lane publishes nothing and its receiver sees
/// the channel close.
pub(crate) struct ReadinessProbe {
    revision: u64,
    origin: u64,
    frames: usize,
    epoch: Option<u64>,
    admitted_chunks: usize,
    admitted_frames: usize,
    verdict: Option<Readiness>,
    report: Option<oneshot::Sender<Readiness>>,
}

impl ReadinessProbe {
    /// A probe for `plan` that needs at least `frames` mapped output frames.
    pub(crate) fn new(plan: &WarpPlan, frames: usize) -> (Self, oneshot::Receiver<Readiness>) {
        let (report, verdict) = oneshot::channel();
        let activation = plan.activation();
        let probe = Self {
            revision: u64::from(activation.revision()),
            origin: activation.source(),
            frames: frames.max(1),
            epoch: None,
            admitted_chunks: 0,
            admitted_frames: 0,
            verdict: None,
            report: Some(report),
        };
        (probe, verdict)
    }

    /// Binds the probe to the decode epoch the lane was positioned in.
    pub(super) const fn bind(&mut self, epoch: u64) {
        self.epoch = Some(epoch);
    }

    /// Counts one chunk admitted to the lane's ring; a chunk from before the
    /// lane was positioned is stale and proves nothing either way.
    pub(super) fn admit(&mut self, meta: &AudioChunkInfo, epoch: u64, preload_chunks: usize) {
        if self.verdict.is_some() {
            return;
        }
        if self.epoch != Some(epoch) {
            return;
        }
        let entered = self.admitted_chunks > 0 || meta.frame_offset == self.origin;
        let mapped = meta.mapping_revision.map(NonZeroU64::get) == Some(self.revision);
        if !entered || !mapped {
            self.verdict = Some(Readiness::Failed);
            return;
        }
        self.admitted_chunks += 1;
        self.admitted_frames = self
            .admitted_frames
            .saturating_add(usize::try_from(meta.frames).unwrap_or(usize::MAX));
        if self.admitted_chunks >= preload_chunks.max(1) && self.admitted_frames >= self.frames {
            self.verdict = Some(Readiness::Ready);
        }
    }

    /// Records that the lane ended or failed; a proven lane stays proven.
    pub(super) fn fail(&mut self) {
        self.verdict.get_or_insert(Readiness::Failed);
    }

    /// Drops the report channel without a verdict.
    pub(super) fn abandon(&mut self) {
        self.report = None;
    }

    /// Delivers a reached verdict, at most once.
    pub(super) fn publish(&mut self) {
        if let Some(verdict) = self.verdict
            && let Some(report) = self.report.take()
        {
            let _ = report.send(verdict);
        }
    }
}

impl Drop for ReadinessProbe {
    fn drop(&mut self) {
        self.publish();
    }
}

/// A worker slot held for one staged lane before anything is opened, so a
/// lane that does not fit is refused before any work starts.
pub(crate) struct StagedSlot(pub(super) PendingTask);
