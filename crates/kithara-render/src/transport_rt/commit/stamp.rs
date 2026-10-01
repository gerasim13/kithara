use std::num::NonZeroU32;

use kithara_signal::{SessionFrame, TransportRevision};

use super::SessionTransportCommit;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransportCommitStamp {
    sample_rate: NonZeroU32,
    previous: Option<SessionTransportCommit>,
    target_frame: SessionFrame,
    next: SessionTransportCommit,
}

impl TransportCommitStamp {
    #[must_use]
    pub const fn sample_rate(self) -> NonZeroU32 {
        self.sample_rate
    }

    #[must_use]
    pub const fn previous(self) -> Option<SessionTransportCommit> {
        self.previous
    }

    #[must_use]
    pub const fn target_frame(self) -> SessionFrame {
        self.target_frame
    }

    #[must_use]
    pub const fn next(self) -> SessionTransportCommit {
        self.next
    }

    #[must_use]
    pub const fn new(
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
            #[must_use]
            pub fn revision(self) -> TransportRevision;
        }
    }
}
