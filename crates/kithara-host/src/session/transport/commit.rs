use std::num::NonZeroU32;

use kithara_warp::{BeatGridId, BeatGridRevision, BeatGridStamp, SessionEpoch, SessionFrame};

use crate::api::{SessionBeat, SessionTransportSnapshot, Tempo, TransportRevision};

#[derive(Clone, Copy, Debug, Eq, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub(crate) struct SessionGridGeneration {
    id: BeatGridId,
    #[field(get, copy, vis = "pub(crate)")]
    epoch: SessionEpoch,
    revision: Option<BeatGridRevision>,
}

impl SessionGridGeneration {
    pub(crate) const fn new(id: BeatGridId) -> Self {
        Self {
            id,
            epoch: SessionEpoch::new(0),
            revision: None,
        }
    }

    pub(crate) fn next_revision(self) -> Result<BeatGridRevision, TransportProcessError> {
        self.revision
            .map_or(Ok(BeatGridRevision::first()), |revision| {
                revision
                    .checked_next()
                    .ok_or(TransportProcessError::SessionGridGenerationExhausted)
            })
    }

    pub(crate) fn commit_revision(&mut self, revision: BeatGridRevision) {
        self.revision = Some(revision);
    }

    pub(crate) fn advance_restart(&mut self) -> Result<(), TransportProcessError> {
        let epoch = u64::from(self.epoch)
            .checked_add(1)
            .map(SessionEpoch::new)
            .ok_or(TransportProcessError::SessionGridGenerationExhausted)?;
        let revision = Some(match self.revision {
            Some(revision) => revision
                .checked_next()
                .ok_or(TransportProcessError::SessionGridGenerationExhausted)?,
            None => BeatGridRevision::first(),
        });
        self.epoch = epoch;
        self.revision = revision;
        Ok(())
    }

    pub(crate) fn stamp(self) -> Result<BeatGridStamp, TransportProcessError> {
        self.revision
            .map(|revision| BeatGridStamp::new(self.id, revision))
            .ok_or(TransportProcessError::MissingSessionGridRevision)
    }

    pub(crate) fn promote(self, observed: Self) -> Result<Self, TransportProcessError> {
        let reserved_stamp = self.stamp()?;
        let observed_stamp = observed.stamp()?;
        if observed_stamp.grid_id() != reserved_stamp.grid_id() {
            return Err(TransportProcessError::SessionGridGenerationMismatch);
        }
        if observed.epoch == self.epoch {
            return if observed_stamp.revision() >= reserved_stamp.revision() {
                Ok(observed)
            } else {
                Err(TransportProcessError::SessionGridGenerationMismatch)
            };
        }
        let mut successor = observed;
        successor.advance_restart()?;
        if successor.epoch != self.epoch {
            return Err(TransportProcessError::SessionGridGenerationMismatch);
        }
        let successor_stamp = successor.stamp()?;
        if successor_stamp.revision() > reserved_stamp.revision() {
            Ok(successor)
        } else {
            Ok(self)
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum TransportBoundary {
    #[default]
    Continuous,
    Relocate(SessionBeat),
}

#[derive(Clone, Copy, Debug, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(get, vis = "pub(crate)")]
pub(crate) struct SessionTransportCommit {
    #[field(get, copy)]
    tempo: Tempo,
    #[field(get, copy)]
    boundary: TransportBoundary,
    #[field(get, copy)]
    revision: TransportRevision,
    #[field(get = is_playing, copy)]
    playing: bool,
}

impl SessionTransportCommit {
    pub(crate) const fn new(tempo: Tempo, playing: bool, revision: TransportRevision) -> Self {
        Self {
            boundary: TransportBoundary::Continuous,
            tempo,
            playing,
            revision,
        }
    }

    pub(crate) const fn relocate(
        tempo: Tempo,
        playing: bool,
        revision: TransportRevision,
        target: SessionBeat,
    ) -> Self {
        Self {
            boundary: TransportBoundary::Relocate(target),
            tempo,
            playing,
            revision,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(get, vis = "pub(crate)")]
pub(crate) struct TransportCommitStamp {
    #[field(get, copy)]
    sample_rate: NonZeroU32,
    #[field(get, copy)]
    previous: Option<SessionTransportCommit>,
    #[field(get, copy)]
    target_frame: SessionFrame,
    #[field(get, copy)]
    next: SessionTransportCommit,
}

impl TransportCommitStamp {
    pub(crate) const fn new(
        previous: Option<SessionTransportCommit>,
        next: SessionTransportCommit,
        target_frame: SessionFrame,
        sample_rate: NonZeroU32,
    ) -> Self {
        Self {
            sample_rate,
            previous,
            target_frame,
            next,
        }
    }

    delegate::delegate! {
        to self.next {
            pub(crate) fn revision(self) -> TransportRevision;
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(get, vis = "pub(crate)")]
pub(crate) enum TransportCommitResult {
    Aborted(#[field(rename = revision)] TransportRevision),
    Applied(#[field(rename = revision)] TransportRevision),
    Rejected(#[field(rename = revision, copy)] TransportRevision),
}

#[derive(Clone, Copy, Debug, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(get, vis = "pub(crate)")]
pub(crate) struct TransportObservation {
    #[field(get, copy)]
    completion: Option<TransportCommitResult>,
    #[field(get, copy)]
    snapshot: Option<SessionTransportSnapshot>,
    #[field(get, copy)]
    session_grid: SessionGridGeneration,
}

impl TransportObservation {
    pub(crate) const fn new(
        completion: Option<TransportCommitResult>,
        snapshot: Option<SessionTransportSnapshot>,
        session_grid: SessionGridGeneration,
    ) -> Self {
        Self {
            completion,
            snapshot,
            session_grid,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum TransportCommitEvent {
    Abort(TransportRevision),
    Apply(TransportRevision),
    Stage(TransportCommitStamp),
}

/// Audio-thread transport failures. The processor logs them through an
/// allocation-free `&'static str`, so `message` is the single source of the
/// text and `Display` forwards to it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum TransportProcessError {
    #[error("{}", Self::AbortMismatch.message())]
    AbortMismatch,
    #[error("{}", Self::DuplicateEvent.message())]
    DuplicateEvent,
    #[error("{}", Self::FrameDiscontinuity.message())]
    FrameDiscontinuity,
    #[error("{}", Self::SessionGridGenerationExhausted.message())]
    SessionGridGenerationExhausted,
    #[error("{}", Self::SessionGridGenerationMismatch.message())]
    SessionGridGenerationMismatch,
    #[error("{}", Self::InvalidBeatRange.message())]
    InvalidBeatRange,
    #[error("{}", Self::MissingSessionGridRevision.message())]
    MissingSessionGridRevision,
    #[error("{}", Self::MissingObservation.message())]
    MissingObservation,
    #[error("{}", Self::MissingState.message())]
    MissingState,
    #[error("{}", Self::UnexpectedEvent.message())]
    UnexpectedEvent,
}

impl TransportProcessError {
    pub(crate) const fn message(self) -> &'static str {
        match self {
            Self::AbortMismatch => "transport abort targets an applied revision",
            Self::DuplicateEvent => "session transport received duplicate events in one block",
            Self::FrameDiscontinuity => "graph render clock is discontinuous",
            Self::SessionGridGenerationExhausted => {
                "session beat grid generation space is exhausted"
            }
            Self::SessionGridGenerationMismatch => {
                "session beat grid generation does not match the reserved route boundary"
            }
            Self::InvalidBeatRange => "session transport produced an invalid beat range",
            Self::MissingSessionGridRevision => {
                "active transport has no session beat grid revision"
            }
            Self::MissingObservation => "transport observation store slot is missing",
            Self::MissingState => "transport commit state store slot is missing",
            Self::UnexpectedEvent => "session transport received an unexpected event",
        }
    }
}
