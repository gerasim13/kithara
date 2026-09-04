use std::sync::atomic::{AtomicU8, Ordering};

use crate::PlayError;

#[repr(u8)]
enum PlayerLifecycleState {
    Open,
    Closing,
    Closed,
}

pub(super) struct PlayerLifecycle {
    state: AtomicU8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CloseAdmission {
    Begin,
    AlreadyClosed,
}

impl PlayerLifecycle {
    pub(super) fn begin_close(&self) -> Result<CloseAdmission, PlayError> {
        match self.state.compare_exchange(
            PlayerLifecycleState::Open as u8,
            PlayerLifecycleState::Closing as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => Ok(CloseAdmission::Begin),
            Err(state) if state == PlayerLifecycleState::Closed as u8 => {
                Ok(CloseAdmission::AlreadyClosed)
            }
            Err(_) => Err(PlayError::Closed),
        }
    }

    pub(super) fn finish_close(&self) {
        self.state
            .store(PlayerLifecycleState::Closed as u8, Ordering::Release);
    }

    pub(super) fn is_closed(&self) -> bool {
        self.state.load(Ordering::Acquire) != PlayerLifecycleState::Open as u8
    }

    pub(super) const fn open() -> Self {
        Self {
            state: AtomicU8::new(PlayerLifecycleState::Open as u8),
        }
    }

    pub(super) fn reopen(&self) {
        self.state
            .store(PlayerLifecycleState::Open as u8, Ordering::Release);
    }
}
