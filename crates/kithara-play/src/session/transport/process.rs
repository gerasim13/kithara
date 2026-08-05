use std::{num::NonZeroU32, ops::Range};

use firewheel::{
    event::ProcEvents,
    node::{ProcInfo, ProcStore},
};
use num_traits::ToPrimitive;
use triple_buffer::Input;

use super::commit::{
    RenderFrame, SessionTransportCommit, TransportBoundary, TransportCommitEvent,
    TransportCommitResult, TransportCommitStamp, TransportObservation, TransportProcessError,
};
use crate::api::{SessionBeat, SessionTransportSnapshot, TransportRevision};

#[derive(Clone, Copy, Debug)]
struct TransportAnchor {
    sample_rate: NonZeroU32,
    frame: RenderFrame,
    beat: SessionBeat,
    commit: SessionTransportCommit,
}

impl TransportAnchor {
    fn beat_at(self, frame: RenderFrame) -> Result<SessionBeat, TransportProcessError> {
        let frames = frame
            .get()
            .checked_sub(self.frame.get())
            .and_then(|value| value.to_f64())
            .ok_or(TransportProcessError::InvalidBeatRange)?;
        let beats = self.beat.get()
            + frames * self.commit.tempo().beats_per_second() / f64::from(self.sample_rate.get());
        SessionBeat::new(beats).map_err(|_| TransportProcessError::InvalidBeatRange)
    }
}

#[derive(Clone, Copy, Debug)]
struct RenderBoundary {
    frame: RenderFrame,
}

#[derive(Debug, Default)]
pub(crate) struct TransportCommitState {
    active: Option<SessionTransportCommit>,
    anchor: Option<TransportAnchor>,
    boundary: Option<RenderBoundary>,
    completion: Option<TransportCommitResult>,
    ignored_through_revision: Option<TransportRevision>,
    pending: Option<TransportCommitStamp>,
    reanchor_beat: Option<SessionBeat>,
    snapshot: Option<SessionTransportSnapshot>,
}

#[derive(Debug)]
pub(crate) struct TransportObservationInput(Input<TransportObservation>);

impl TransportObservationInput {
    pub(crate) const fn new(input: Input<TransportObservation>) -> Self {
        Self(input)
    }

    delegate::delegate! {
        to self.0 {
            fn write(&mut self, observation: TransportObservation);
        }
    }
}

pub(crate) fn process_transport(
    info: &ProcInfo,
    events: &mut ProcEvents,
    store: &mut ProcStore,
) -> Result<(), TransportProcessError> {
    let result = store
        .try_get_mut::<TransportCommitState>()
        .ok_or(TransportProcessError::MissingState)?
        .process(info, events);
    if let Err(error) = result {
        store
            .try_get_mut::<TransportCommitState>()
            .ok_or(TransportProcessError::MissingState)?
            .reject_pending();
        publish_observation(store)?;
        return Err(error);
    }
    publish_observation(store)?;
    result
}

pub(crate) fn restart_transport(store: &mut ProcStore) -> Result<(), TransportProcessError> {
    store
        .try_get_mut::<TransportCommitState>()
        .ok_or(TransportProcessError::MissingState)?
        .restart();
    publish_observation(store)
}

fn publish_observation(store: &mut ProcStore) -> Result<(), TransportProcessError> {
    let observation = {
        let state = store
            .try_get::<TransportCommitState>()
            .ok_or(TransportProcessError::MissingState)?;
        TransportObservation::new(state.completion, state.snapshot)
    };
    store
        .try_get_mut::<TransportObservationInput>()
        .ok_or(TransportProcessError::MissingObservation)?
        .write(observation);
    Ok(())
}

impl TransportCommitState {
    fn apply_abort(&mut self, revision: TransportRevision) -> Result<(), TransportProcessError> {
        if self
            .ignored_through_revision
            .is_some_and(|ignored| ignored >= revision)
            || self.completion == Some(TransportCommitResult::Aborted(revision))
        {
            self.completion = Some(TransportCommitResult::Aborted(revision));
            return Ok(());
        }
        if self
            .active
            .is_some_and(|active| active.revision() >= revision)
        {
            return Err(TransportProcessError::AbortMismatch);
        }
        if self
            .pending
            .is_some_and(|stamp| stamp.revision() == revision)
        {
            self.pending = None;
        }
        self.ignored_through_revision = Some(revision);
        self.completion = Some(TransportCommitResult::Aborted(revision));
        Ok(())
    }

    fn apply_commit(
        &mut self,
        info: &ProcInfo,
        revision: TransportRevision,
    ) -> Result<(), TransportProcessError> {
        if self
            .ignored_through_revision
            .is_some_and(|ignored| revision <= ignored)
            || self
                .active
                .is_some_and(|active| active.revision() >= revision)
        {
            return Ok(());
        }
        let Some(stamp) = self.pending.filter(|stamp| stamp.revision() == revision) else {
            self.reject_revision(revision);
            return Ok(());
        };
        if stamp.previous() != self.active
            || stamp.sample_rate() != info.sample_rate
            || stamp.target_frame().get() != info.clock_samples.0
        {
            self.reject_revision(revision);
            return Ok(());
        }
        let beat = match (stamp.next().boundary(), stamp.previous()) {
            (TransportBoundary::Relocate(target), _) => target,
            (TransportBoundary::Continuous, Some(previous)) => {
                let anchor = self.anchor.ok_or(TransportProcessError::InvalidBeatRange)?;
                if previous.is_playing() {
                    anchor.beat_at(stamp.target_frame())?
                } else {
                    anchor.beat
                }
            }
            (TransportBoundary::Continuous, None) => {
                SessionBeat::new(0.0).map_err(|_| TransportProcessError::InvalidBeatRange)?
            }
        };
        self.active = Some(stamp.next());
        self.anchor = Some(TransportAnchor {
            beat,
            commit: stamp.next(),
            frame: stamp.target_frame(),
            sample_rate: stamp.sample_rate(),
        });
        self.pending = None;
        self.completion = Some(TransportCommitResult::Applied(revision));
        Ok(())
    }

    fn apply_events(
        &mut self,
        info: &ProcInfo,
        events: &mut ProcEvents,
    ) -> Result<(), TransportProcessError> {
        let mut abort = None;
        let mut apply = None;
        let mut stage = None;
        for event in events.drain() {
            let event = event
                .downcast_ref::<TransportCommitEvent>()
                .copied()
                .ok_or(TransportProcessError::UnexpectedEvent)?;
            match event {
                TransportCommitEvent::Abort(revision) => {
                    Self::set_once(&mut abort, revision)?;
                }
                TransportCommitEvent::Apply(revision) => {
                    Self::set_once(&mut apply, revision)?;
                }
                TransportCommitEvent::Stage(stamp) => {
                    Self::set_once(&mut stage, stamp)?;
                }
            }
        }
        if let Some(revision) = abort {
            self.apply_abort(revision)?;
        }
        if let Some(stamp) = stage {
            self.apply_stage(info, stamp);
        }
        if let Some(revision) = apply {
            self.apply_commit(info, revision)?;
        }
        Ok(())
    }

    fn apply_stage(&mut self, info: &ProcInfo, stamp: TransportCommitStamp) {
        let revision = stamp.revision();
        if self
            .ignored_through_revision
            .is_some_and(|ignored| revision <= ignored)
        {
            return;
        }
        if self.pending.is_some()
            || stamp.previous() != self.active
            || stamp.sample_rate() != info.sample_rate
            || stamp.target_frame().get() < info.clock_samples.0
        {
            self.reject_revision(revision);
            return;
        }
        self.pending = Some(stamp);
        self.completion = None;
    }

    fn next_boundary(
        info: &ProcInfo,
        active: Option<SessionTransportCommit>,
        current: Option<RenderBoundary>,
    ) -> Result<Option<RenderBoundary>, TransportProcessError> {
        if active.is_none() {
            return Ok(current);
        }
        let frames =
            i64::try_from(info.frames).map_err(|_| TransportProcessError::InvalidBeatRange)?;
        let frame = info
            .clock_samples
            .0
            .checked_add(frames)
            .ok_or(TransportProcessError::InvalidBeatRange)?;
        Ok(Some(RenderBoundary {
            frame: RenderFrame::new(frame),
        }))
    }

    fn next_snapshot(
        &self,
        session_beats: Option<&Range<SessionBeat>>,
    ) -> Result<Option<SessionTransportSnapshot>, TransportProcessError> {
        let Some(commit) = self.active else {
            return Ok(self.snapshot);
        };
        let position = if commit.is_playing() {
            session_beats
                .ok_or(TransportProcessError::InvalidBeatRange)?
                .end
        } else {
            self.anchor
                .ok_or(TransportProcessError::InvalidBeatRange)?
                .beat
        };
        Ok(Some(SessionTransportSnapshot::new(
            position,
            commit.is_playing(),
            commit.tempo(),
            commit.revision(),
        )))
    }

    fn process(
        &mut self,
        info: &ProcInfo,
        events: &mut ProcEvents,
    ) -> Result<(), TransportProcessError> {
        self.reanchor(info)?;
        self.validate_frame(info)?;
        self.apply_events(info, events)?;
        let session_beats = self.session_beats(info)?;
        self.boundary = Self::next_boundary(info, self.active, self.boundary)?;
        self.snapshot = self.next_snapshot(session_beats.as_ref())?;
        Ok(())
    }

    fn reanchor(&mut self, info: &ProcInfo) -> Result<(), TransportProcessError> {
        let Some(beat) = self.reanchor_beat.take() else {
            return Ok(());
        };
        let commit = self.active.ok_or(TransportProcessError::InvalidBeatRange)?;
        self.anchor = Some(TransportAnchor {
            beat,
            commit,
            frame: RenderFrame::new(info.clock_samples.0),
            sample_rate: info.sample_rate,
        });
        self.boundary = None;
        Ok(())
    }

    fn reject_pending(&mut self) {
        if let Some(stamp) = self.pending.take() {
            self.reject_revision(stamp.revision());
        }
    }

    fn reject_revision(&mut self, revision: TransportRevision) {
        if self
            .pending
            .is_some_and(|stamp| stamp.revision() == revision)
        {
            self.pending = None;
        }
        self.ignored_through_revision = Some(
            self.ignored_through_revision
                .map_or(revision, |ignored| ignored.max(revision)),
        );
        self.completion = Some(TransportCommitResult::Rejected(revision));
    }

    fn restart(&mut self) {
        self.reject_pending();
        self.reanchor_beat = self.snapshot.map(|snapshot| snapshot.position());
        self.anchor = None;
        self.boundary = None;
    }

    fn session_beats(
        &self,
        info: &ProcInfo,
    ) -> Result<Option<Range<SessionBeat>>, TransportProcessError> {
        let Some(active) = self.active else {
            return Ok(None);
        };
        if !active.is_playing() {
            return Ok(None);
        }
        let anchor = self.anchor.ok_or(TransportProcessError::InvalidBeatRange)?;
        let frames =
            i64::try_from(info.frames).map_err(|_| TransportProcessError::InvalidBeatRange)?;
        let start = RenderFrame::new(info.clock_samples.0);
        let end = RenderFrame::new(
            info.clock_samples
                .0
                .checked_add(frames)
                .ok_or(TransportProcessError::InvalidBeatRange)?,
        );
        Ok(Some(anchor.beat_at(start)?..anchor.beat_at(end)?))
    }

    fn set_once<T>(slot: &mut Option<T>, value: T) -> Result<(), TransportProcessError> {
        if slot.replace(value).is_some() {
            return Err(TransportProcessError::DuplicateEvent);
        }
        Ok(())
    }

    fn validate_frame(&self, info: &ProcInfo) -> Result<(), TransportProcessError> {
        if let Some(anchor) = self.anchor
            && anchor.sample_rate != info.sample_rate
        {
            return Err(TransportProcessError::FrameDiscontinuity);
        }
        if let Some(boundary) = self.boundary
            && boundary.frame.get() != info.clock_samples.0
        {
            return Err(TransportProcessError::FrameDiscontinuity);
        }
        Ok(())
    }
}
