use kithara_signal::SessionEpoch;
use kithara_warp::{BeatGridId, BeatGridRevision, BeatGridStamp};

use crate::api::SessionTransportSnapshot;

#[derive(Clone, Copy, Debug, Eq, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub(crate) struct SessionGridGeneration {
    id: BeatGridId,
    revision: Option<BeatGridRevision>,
    #[field(get, copy, vis = "pub(crate)")]
    epoch: SessionEpoch,
}

impl SessionGridGeneration {
    pub(crate) const fn new(id: BeatGridId) -> Self {
        Self {
            id,
            epoch: SessionEpoch::new(0),
            revision: None,
        }
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

    pub(crate) fn commit_revision(&mut self, revision: BeatGridRevision) {
        self.revision = Some(revision);
    }

    pub(crate) fn next_revision(self) -> Result<BeatGridRevision, TransportProcessError> {
        self.revision
            .map_or(Ok(BeatGridRevision::first()), |revision| {
                revision
                    .checked_next()
                    .ok_or(TransportProcessError::SessionGridGenerationExhausted)
            })
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

    pub(crate) fn stamp(self) -> Result<BeatGridStamp, TransportProcessError> {
        self.revision
            .map(|revision| BeatGridStamp::new(self.id, revision))
            .ok_or(TransportProcessError::MissingSessionGridRevision)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(get, vis = "pub(crate)")]
pub(crate) struct TransportObservation {
    #[field(get, copy)]
    snapshot: Option<SessionTransportSnapshot>,
    #[field(get, copy)]
    session_grid: SessionGridGeneration,
}

impl TransportObservation {
    pub(crate) const fn new(
        snapshot: Option<SessionTransportSnapshot>,
        session_grid: SessionGridGeneration,
    ) -> Self {
        Self {
            snapshot,
            session_grid,
        }
    }
}

/// Audio-thread transport failures. The processor logs them through an
/// allocation-free `&'static str`, so `message` is the single source of the
/// text and `Display` forwards to it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum TransportProcessError {
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
    #[error("{}", Self::RevisionExhausted.message())]
    RevisionExhausted,
}

impl TransportProcessError {
    pub(crate) const fn message(self) -> &'static str {
        match self {
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
            Self::MissingState => "transport state store slot is missing",
            Self::RevisionExhausted => "session transport revision space is exhausted",
        }
    }
}
