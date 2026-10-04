use std::{num::NonZeroU32, ops::Range};

use firewheel::node::{ProcInfo, ProcStore};
use kithara_command::{Due, Inbox};
use kithara_config::LiveConfig;
use kithara_signal::{SessionEpoch, SessionFrame};
use kithara_warp::{SessionAnchor, SessionBeat};
use triple_buffer::Input;

use super::commit::{SessionGridGeneration, TransportObservation, TransportProcessError};
use crate::{
    api::{SessionTransportSnapshot, Tempo, TransportRevision},
    consts,
    host::{HostSettings, HostSettingsChange},
    session::queue::{HostPart, HostProtocol},
};

#[derive(Debug)]
pub(super) struct TransportFrame {
    pub(super) trajectory: SessionAnchor,
    pub(super) transport_revision: TransportRevision,
    pub(super) session_epoch: SessionEpoch,
}

/// The transport's half of the Host queue: the settings as the render graph
/// applied them, the beat anchor they put on the render clock, and the inbox
/// the session owner sends changes through.
pub(crate) struct TransportState {
    inbox: Inbox<HostProtocol>,
    settings: HostSettings,
    anchor: Option<SessionAnchor>,
    boundary: Option<SessionFrame>,
    reanchor_beat: Option<SessionBeat>,
    revision: TransportRevision,
    snapshot: Option<SessionTransportSnapshot>,
    session_grid: SessionGridGeneration,
}

/// What one due batch leaves behind once every command of it applied.
#[derive(Clone, Copy)]
struct Staged {
    settings: HostSettings,
    anchor: Option<SessionAnchor>,
    retargeted: bool,
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

pub(super) fn process_transport(
    info: &ProcInfo,
    store: &mut ProcStore,
) -> Result<TransportFrame, TransportProcessError> {
    let result = store
        .try_get_mut::<TransportState>()
        .ok_or(TransportProcessError::MissingState)?
        .process(info);
    publish_observation(store)?;
    result
}

pub(crate) fn restart_transport(store: &mut ProcStore) -> Result<(), TransportProcessError> {
    let result = store
        .try_get_mut::<TransportState>()
        .ok_or(TransportProcessError::MissingState)?
        .restart();
    publish_observation(store)?;
    result
}

pub(crate) fn converge_transport_restart(
    store: &mut ProcStore,
    target: SessionGridGeneration,
) -> Result<SessionGridGeneration, TransportProcessError> {
    let result = store
        .try_get_mut::<TransportState>()
        .ok_or(TransportProcessError::MissingState)?
        .converge_restart(target);
    publish_observation(store)?;
    result
}

fn publish_observation(store: &mut ProcStore) -> Result<(), TransportProcessError> {
    let observation = {
        let state = store
            .try_get::<TransportState>()
            .ok_or(TransportProcessError::MissingState)?;
        TransportObservation::new(state.snapshot, state.session_grid)
    };
    store
        .try_get_mut::<TransportObservationInput>()
        .ok_or(TransportProcessError::MissingObservation)?
        .write(observation);
    Ok(())
}

impl TransportState {
    pub(crate) const fn new(
        inbox: Inbox<HostProtocol>,
        settings: HostSettings,
        session_grid: SessionGridGeneration,
    ) -> Self {
        Self {
            inbox,
            settings,
            session_grid,
            anchor: None,
            boundary: None,
            reanchor_beat: None,
            revision: TransportRevision::first(),
            snapshot: None,
        }
    }

    /// Puts the beat anchor on the first block of a stream: session beat 0 on
    /// a fresh transport, the beat a restart stopped on otherwise.
    fn anchor_block(&mut self, info: &ProcInfo) -> Result<(), TransportProcessError> {
        if self.anchor.is_some() {
            return Ok(());
        }
        let beat = match self.reanchor_beat.take() {
            Some(beat) => beat,
            None => SessionBeat::new(0.0).map_err(|_| TransportProcessError::InvalidBeatRange)?,
        };
        let anchor = Self::build_anchor(
            SessionFrame::new(info.clock_samples.0),
            beat,
            self.settings.tempo(),
            info.sample_rate,
        )?;
        let revision = self.session_grid.next_revision()?;
        self.anchor = Some(anchor);
        self.boundary = None;
        self.session_grid.commit_revision(revision);
        Ok(())
    }

    /// Applies the batches due inside the block in time order. A batch
    /// applies whole or not at all: its commands stage on copies, and only a
    /// batch whose every command staged moves the transport. Only a batch
    /// that re-anchors the beats takes a new transport revision. A block that
    /// does not follow the last one refuses every tempo change due in it with
    /// `continuity`'s error, since no beat anchor is known on its frames.
    fn apply_due(&mut self, info: &ProcInfo, continuity: Result<(), TransportProcessError>) {
        let Self {
            inbox,
            settings,
            anchor,
            revision,
            session_grid,
            ..
        } = self;
        inbox.drain();
        let start = SessionFrame::new(info.clock_samples.0);
        while let Some(due) = inbox.next_due(start, info.frames) {
            let staged = Self::stage(&due, *settings, *anchor, continuity).and_then(|staged| {
                if !staged.retargeted {
                    return Ok((staged, *revision, None));
                }
                let next = revision
                    .checked_next()
                    .ok_or(TransportProcessError::RevisionExhausted)?;
                Ok((staged, next, Some(session_grid.next_revision()?)))
            });
            match staged {
                Ok((staged, next, grid)) => {
                    *settings = staged.settings;
                    *anchor = staged.anchor;
                    *revision = next;
                    if let Some(grid) = grid {
                        session_grid.commit_revision(grid);
                    }
                    due.apply(next);
                }
                Err(error) => due.refuse(error),
            }
        }
    }

    fn build_anchor(
        frame: SessionFrame,
        beat: SessionBeat,
        tempo: Tempo,
        sample_rate: NonZeroU32,
    ) -> Result<SessionAnchor, TransportProcessError> {
        let anchor = SessionAnchor::new(frame, beat, tempo.beats_per_second(), sample_rate)
            .map_err(|_| TransportProcessError::InvalidBeatRange)?;
        if anchor
            .frame_at(beat)
            .map_err(|_| TransportProcessError::InvalidBeatRange)?
            != frame
        {
            return Err(TransportProcessError::InvalidBeatRange);
        }
        Ok(anchor)
    }

    fn converge_restart(
        &mut self,
        target: SessionGridGeneration,
    ) -> Result<SessionGridGeneration, TransportProcessError> {
        let target_stamp = target.stamp()?;
        let current_stamp = self.session_grid.stamp()?;
        if current_stamp.grid_id() != target_stamp.grid_id() {
            return Err(TransportProcessError::SessionGridGenerationMismatch);
        }
        if self.session_grid.epoch() < target.epoch() {
            let mut successor = self.session_grid;
            successor.advance_restart()?;
            if successor.epoch() != target.epoch() {
                return Err(TransportProcessError::SessionGridGenerationMismatch);
            }
            self.restart()?;
        } else if self.session_grid.epoch() > target.epoch() {
            return Err(TransportProcessError::SessionGridGenerationMismatch);
        }
        let actual_stamp = self.session_grid.stamp()?;
        if self.session_grid.epoch() == target.epoch()
            && actual_stamp.revision() >= target_stamp.revision()
        {
            Ok(self.session_grid)
        } else {
            Err(TransportProcessError::SessionGridGenerationMismatch)
        }
    }

    fn process(&mut self, info: &ProcInfo) -> Result<TransportFrame, TransportProcessError> {
        self.anchor_block(info)?;
        let continuity = self.validate_frame(info);
        self.apply_due(info, continuity);
        continuity?;
        let anchor = self.anchor.ok_or(TransportProcessError::InvalidBeatRange)?;
        let (frames, beats) = Self::block_span(anchor, info)?;
        self.boundary = Some(frames.end);
        self.snapshot = Some(SessionTransportSnapshot::new(
            beats.end,
            self.settings.tempo(),
            self.revision,
            anchor,
            self.session_grid.stamp()?,
            self.session_grid.epoch(),
        ));
        Ok(TransportFrame {
            trajectory: anchor,
            session_epoch: self.session_grid.epoch(),
            transport_revision: self.revision,
        })
    }

    fn restart(&mut self) -> Result<(), TransportProcessError> {
        if let Some(snapshot) = self.snapshot.take() {
            self.reanchor_beat = Some(snapshot.position());
        }
        let generation = self.session_grid.advance_restart();
        if generation.is_err() {
            self.reanchor_beat = None;
        }
        self.anchor = None;
        self.boundary = None;
        generation
    }

    /// The block's frame range and the session beats it covers.
    fn block_span(
        anchor: SessionAnchor,
        info: &ProcInfo,
    ) -> Result<(Range<SessionFrame>, Range<SessionBeat>), TransportProcessError> {
        let frames =
            i64::try_from(info.frames).map_err(|_| TransportProcessError::InvalidBeatRange)?;
        let start = SessionFrame::new(info.clock_samples.0);
        let end = SessionFrame::new(
            info.clock_samples
                .0
                .checked_add(frames)
                .ok_or(TransportProcessError::InvalidBeatRange)?,
        );
        let at = |frame| {
            anchor
                .beat_at(frame)
                .map_err(|_| TransportProcessError::InvalidBeatRange)
        };
        Ok((start..end, at(start)?..at(end)?))
    }

    fn stage(
        due: &Due<'_, HostProtocol>,
        settings: HostSettings,
        anchor: Option<SessionAnchor>,
        continuity: Result<(), TransportProcessError>,
    ) -> Result<Staged, TransportProcessError> {
        let mut staged = Staged {
            settings,
            anchor,
            retargeted: false,
        };
        for command in due.commands() {
            let HostPart::Settings(change) = *command;
            let HostSettingsChange::Tempo(tempo) = change;
            continuity?;
            if tempo == staged.settings.tempo() {
                continue;
            }
            staged.settings.apply_change(change);
            staged.anchor = Some(
                staged
                    .anchor
                    .ok_or(TransportProcessError::InvalidBeatRange)?
                    .retarget(
                        due.at(),
                        tempo.beats_per_second(),
                        consts::TEMPO_SMOOTH_SECONDS,
                    )
                    .map_err(|_| TransportProcessError::InvalidBeatRange)?,
            );
            staged.retargeted = true;
        }
        Ok(staged)
    }

    fn validate_frame(&self, info: &ProcInfo) -> Result<(), TransportProcessError> {
        if let Some(anchor) = self.anchor
            && anchor.sample_rate() != info.sample_rate
        {
            return Err(TransportProcessError::FrameDiscontinuity);
        }
        if let Some(boundary) = self.boundary
            && i64::from(boundary) != info.clock_samples.0
        {
            return Err(TransportProcessError::FrameDiscontinuity);
        }
        Ok(())
    }
}
