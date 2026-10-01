use kithara_signal::TransportRevision;
use kithara_warp::SessionBeat;

use super::{SessionGridGeneration, TransportCommitStamp};
use crate::transport::{SessionTransportSnapshot, Tempo};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum TransportBoundary {
    #[default]
    Continuous,
    Relocate(SessionBeat),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SessionTransportCommit {
    tempo: Tempo,
    boundary: TransportBoundary,
    revision: TransportRevision,
    playing: bool,
}

impl SessionTransportCommit {
    #[must_use]
    pub const fn tempo(self) -> Tempo {
        self.tempo
    }

    #[must_use]
    pub const fn boundary(self) -> TransportBoundary {
        self.boundary
    }

    #[must_use]
    pub const fn revision(self) -> TransportRevision {
        self.revision
    }

    #[must_use]
    pub const fn is_playing(self) -> bool {
        self.playing
    }

    #[must_use]
    pub const fn new(tempo: Tempo, playing: bool, revision: TransportRevision) -> Self {
        Self {
            tempo,
            playing,
            revision,
            boundary: TransportBoundary::Continuous,
        }
    }

    #[must_use]
    pub const fn relocate(
        tempo: Tempo,
        playing: bool,
        revision: TransportRevision,
        target: SessionBeat,
    ) -> Self {
        Self {
            tempo,
            playing,
            revision,
            boundary: TransportBoundary::Relocate(target),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransportCommitResult {
    Aborted(TransportRevision),
    Applied(TransportRevision),
    Rejected(TransportRevision),
}

impl TransportCommitResult {
    #[must_use]
    pub const fn revision(self) -> TransportRevision {
        match self {
            Self::Aborted(revision) | Self::Applied(revision) | Self::Rejected(revision) => {
                revision
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransportObservation {
    completion: Option<TransportCommitResult>,
    snapshot: Option<SessionTransportSnapshot>,
    session_grid: SessionGridGeneration,
}

impl TransportObservation {
    #[must_use]
    pub const fn completion(self) -> Option<TransportCommitResult> {
        self.completion
    }

    #[must_use]
    pub const fn snapshot(self) -> Option<SessionTransportSnapshot> {
        self.snapshot
    }

    #[must_use]
    pub const fn session_grid(self) -> SessionGridGeneration {
        self.session_grid
    }

    #[must_use]
    pub const fn new(
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
pub enum TransportCommitEvent {
    Abort(TransportRevision),
    Apply(TransportRevision),
    Stage(TransportCommitStamp),
}

/// Audio-thread transport failures. The processor logs them through an
/// allocation-free `&'static str`, so `message` is the single source of the
/// text and `Display` forwards to it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum TransportProcessError {
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
    #[must_use]
    pub const fn message(self) -> &'static str {
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
