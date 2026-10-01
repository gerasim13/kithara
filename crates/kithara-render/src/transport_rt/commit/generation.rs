use kithara_signal::SessionEpoch;
use kithara_warp::{BeatGridId, BeatGridRevision, BeatGridStamp};

use super::TransportProcessError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionGridGeneration {
    id: BeatGridId,
    revision: Option<BeatGridRevision>,
    epoch: SessionEpoch,
}

impl SessionGridGeneration {
    #[must_use]
    pub const fn epoch(self) -> SessionEpoch {
        self.epoch
    }

    #[must_use]
    pub const fn new(id: BeatGridId) -> Self {
        Self {
            id,
            epoch: SessionEpoch::new(0),
            revision: None,
        }
    }

    pub fn advance_restart(&mut self) -> Result<(), TransportProcessError> {
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

    pub fn commit_revision(&mut self, revision: BeatGridRevision) {
        self.revision = Some(revision);
    }

    pub fn next_revision(self) -> Result<BeatGridRevision, TransportProcessError> {
        self.revision
            .map_or(Ok(BeatGridRevision::first()), |revision| {
                revision
                    .checked_next()
                    .ok_or(TransportProcessError::SessionGridGenerationExhausted)
            })
    }

    pub fn promote(self, observed: Self) -> Result<Self, TransportProcessError> {
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

    pub fn stamp(self) -> Result<BeatGridStamp, TransportProcessError> {
        self.revision
            .map(|revision| BeatGridStamp::new(self.id, revision))
            .ok_or(TransportProcessError::MissingSessionGridRevision)
    }
}
