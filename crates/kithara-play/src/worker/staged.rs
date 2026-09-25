use std::num::NonZeroU64;

use kithara_platform::tokio::sync::oneshot;
use kithara_signal::AudioChunkInfo;
use kithara_warp::WarpPlan;
use kithara_worker::PendingTask;

/// Verdict a staged lane owes the preparation it was opened for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Readiness {
    /// The lane's ring holds mapped PCM from the plan's activation on, as
    /// many chunks as the playback path preloads before it sounds.
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
    epoch: Option<u64>,
    admitted_chunks: usize,
    admitted_frames: usize,
    verdict: Option<Readiness>,
    report: Option<oneshot::Sender<Readiness>>,
}

impl ReadinessProbe {
    /// A probe for the lane entering `plan`.
    pub(crate) fn new(plan: &WarpPlan) -> (Self, oneshot::Receiver<Readiness>) {
        let (report, verdict) = oneshot::channel();
        let activation = plan.activation();
        let probe = Self {
            revision: u64::from(activation.revision()),
            origin: activation.source(),
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
        if self.admitted_chunks >= preload_chunks.max(1) && self.admitted_frames > 0 {
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

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use kithara_beat::{
        BeatGridModel, BeatGridState as WireState, GridBeat, RawBeatGrid, SCHEMA_VERSION,
    };
    use kithara_signal::{SessionEpoch, SessionFrame};
    use kithara_test_utils::kithara;
    use kithara_warp::{
        AssetAxis, AssetExtent, Beat, BeatAlignment, BeatGridId, BeatGridRevision,
        BeatGridSnapshot, MapPoint, SessionAnchor, SessionBeat, WarpMap, WarpMapRevision,
    };

    use super::*;

    mod consts {
        /// Chunks the playback path preloads before a lane sounds.
        pub(super) const PRELOAD: usize = 2;
        /// Decode epoch the lane was positioned in.
        pub(super) const POSITIONED: u64 = 3;
    }

    /// A plan entering a 120 BPM recording one second into a session at the
    /// same tempo, so the activation starts past the recording's first frame.
    fn plan() -> WarpPlan {
        let rate = NonZeroU32::new(48_000).expect("fixture rate");
        let model = BeatGridModel::try_from(RawBeatGrid {
            schema_version: SCHEMA_VERSION,
            model_id: "readiness".to_owned(),
            revision: 1,
            state: WireState::Final,
            duration: Some(10.0),
            bpm: 120.0,
            beats: [0.0, 0.5]
                .into_iter()
                .zip(0..)
                .map(|(at, ordinal)| GridBeat {
                    at,
                    ordinal,
                    confidence: Some(1.0),
                })
                .collect(),
            downbeats: Vec::new(),
            meter: None,
        })
        .expect("fixture model");
        let asset = BeatGridSnapshot::model(
            BeatGridId::allocate().expect("asset identity"),
            BeatGridRevision::first(),
            &model,
            AssetAxis::new(rate, AssetExtent::Bounded(480_000)),
        )
        .expect("fixture asset grid");
        let session = BeatGridSnapshot::session(
            BeatGridId::allocate().expect("session identity"),
            BeatGridRevision::first(),
            SessionEpoch::new(0),
            SessionAnchor::new(SessionFrame::new(0), SessionBeat::default(), 2.0, rate)
                .expect("fixture tempo"),
            None,
        );
        let cue = Beat::new(0.0).expect("fixture cue");
        let alignment = BeatAlignment::new(
            MapPoint::new(asset.stamp(), cue),
            MapPoint::new(session.stamp(), cue),
        );
        let map = WarpMap::projected(asset, session, alignment, WarpMapRevision::first())
            .expect("fixture projection");
        WarpPlan::new(map, SessionFrame::new(48_000)).expect("fixture activation")
    }

    fn chunk(frame_offset: u64, revision: Option<NonZeroU64>) -> AudioChunkInfo {
        AudioChunkInfo {
            frames: 1_024,
            frame_offset,
            mapping_revision: revision,
            ..AudioChunkInfo::default()
        }
    }

    /// The exact entry of `plan` and the chunk that follows it.
    fn entry(plan: &WarpPlan) -> [AudioChunkInfo; consts::PRELOAD] {
        let activation = plan.activation();
        let revision = NonZeroU64::new(u64::from(activation.revision()));
        let first = chunk(activation.source(), revision);
        let next = chunk(first.frame_offset + u64::from(first.frames), revision);
        [first, next]
    }

    #[kithara::test]
    fn a_chunk_from_another_decode_epoch_proves_nothing() {
        let plan = plan();
        let (mut probe, _report) = ReadinessProbe::new(&plan);
        let stale = chunk(0, None);

        probe.admit(&stale, consts::POSITIONED, consts::PRELOAD);
        probe.bind(consts::POSITIONED);
        probe.admit(&stale, consts::POSITIONED - 1, consts::PRELOAD);
        assert_eq!(
            probe.verdict, None,
            "PCM decoded before the lane was positioned neither proves nor fails it"
        );

        for chunk in entry(&plan) {
            probe.admit(&chunk, consts::POSITIONED, consts::PRELOAD);
        }
        assert_eq!(probe.verdict, Some(Readiness::Ready));
    }

    #[kithara::test]
    #[case::before_the_origin(-1, 0)]
    #[case::after_the_origin(1, 0)]
    #[case::from_another_mapping(0, 1)]
    fn readiness_needs_the_exact_origin_and_revision_of_the_activation(
        #[case] offset: i64,
        #[case] revision_shift: u64,
    ) {
        let plan = plan();
        let activation = plan.activation();
        assert_ne!(
            activation.source(),
            0,
            "the fixture activates mid-recording"
        );
        let (mut probe, _report) = ReadinessProbe::new(&plan);
        probe.bind(consts::POSITIONED);
        let [first, next] = entry(&plan);
        let displaced = chunk(
            first.frame_offset.saturating_add_signed(offset),
            first
                .mapping_revision
                .and_then(|revision| revision.checked_add(revision_shift)),
        );

        probe.admit(&displaced, consts::POSITIONED, consts::PRELOAD);
        probe.admit(&next, consts::POSITIONED, consts::PRELOAD);
        assert_eq!(
            probe.verdict,
            Some(Readiness::Failed),
            "a lane that does not enter at {} under revision {:?} cannot present the plan",
            activation.source(),
            activation.revision()
        );
    }
}
