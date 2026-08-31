use std::fmt;

use kithara_audio::SeekOutcome;
use kithara_platform::maybe_send::{MaybeSend, MaybeSync};
use kithara_warp::{
    BeatGrid, BeatGridId, BeatGridSnapshot, SyncAdmission, SyncApplied, SyncError, SyncGroup,
    SyncGroupSnapshot, SyncOperation, SyncRejected, SyncStatusSnapshot,
};

use super::{PlaybackView, PlayerImpl, PlayerRuntime};
use crate::{PlayError, SessionBinding};

#[cfg(not(target_arch = "wasm32"))]
#[path = "protocol/native.rs"]
mod target;
#[cfg(target_arch = "wasm32")]
#[path = "protocol/wasm.rs"]
mod target;

pub use target::PlayerMember;
pub(crate) use target::PlayerSync;

impl fmt::Debug for PlayerMember {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PlayerMember")
            .field("grid_id", &self.id())
            .finish_non_exhaustive()
    }
}

/// Canonical object-safe protocol implemented by a standalone player and its
/// orchestration decorators.
///
/// Queue-specific item, EQ, volume, and event APIs remain on their concrete
/// facade. This contract contains only playback operations shared by every
/// host member plus the synchronization-group protocol.
pub trait Player:
    BeatGrid + SyncGroup<NestedGroup = PlayerMember> + MaybeSend + MaybeSync + 'static
{
    /// Start or resume playback.
    fn play(&self);

    /// Pause playback.
    fn pause(&self);

    /// Seek within the current item.
    fn seek_seconds(&self, seconds: f64) -> Result<SeekOutcome, PlayError>;

    /// Advance control-plane and audio-backend work.
    fn tick(&self) -> Result<(), PlayError>;

    /// Read one coherent playback view.
    fn playback_view(&self) -> PlaybackView;

    /// Commit the host-applied deck level after a validated graph batch.
    fn set_host_level(&self, level: f32);

    /// Read the desired host-applied deck level.
    fn host_level(&self) -> f32;

    /// Stop owned work and detach the player from its playback session.
    fn close(&mut self) -> Result<(), PlayError>;
}

/// Produces a cloneable command capability without sharing player identity or
/// synchronization topology.
pub trait PlayerControlSource: Player {
    /// Concrete command capability retained by typed host-owned handles.
    type Control: Clone + MaybeSend + MaybeSync + 'static;

    /// Creates a command capability for this player.
    fn control(&self) -> Self::Control;

    /// Attaches the resident Player to its canonical session exactly once.
    fn attach_session(&mut self, binding: SessionBinding) -> Result<(), PlayError>;

    /// Closes the resident player through a previously issued capability.
    fn close_control(control: &Self::Control) -> Result<(), PlayError>;

    /// Transfers only the sendable Host-owned part of a wasm player.
    #[cfg(target_arch = "wasm32")]
    #[doc(hidden)]
    fn take_host_member(&mut self) -> Result<PlayerMember, PlayError>;
}

impl BeatGrid for PlayerImpl {
    delegate::delegate! {
        to self.sync {
            fn id(&self) -> BeatGridId;
            fn snapshot(&self) -> BeatGridSnapshot;
        }
    }
}

impl SyncGroup for PlayerImpl {
    type NestedGroup = PlayerMember;

    delegate::delegate! {
        to self.sync {
            fn topology(&self) -> Result<SyncGroupSnapshot, SyncError>;
            fn transact(
                &mut self,
                operation: SyncOperation<PlayerMember>,
            ) -> Result<SyncAdmission, SyncRejected<PlayerMember>>;
            fn acknowledge(&mut self, applied: SyncApplied) -> Result<SyncStatusSnapshot, SyncError>;
        }
    }

    fn status(&self) -> SyncStatusSnapshot {
        SyncGroup::status(&self.sync)
    }
}

impl Player for PlayerImpl {
    fn play(&self) {
        let _ = self.runtime.with_open(PlayerRuntime::play);
    }

    fn pause(&self) {
        let _ = self.runtime.with_open(PlayerRuntime::pause);
    }

    fn seek_seconds(&self, seconds: f64) -> Result<SeekOutcome, PlayError> {
        self.runtime
            .with_open_result(|runtime| runtime.seek_seconds(seconds))
    }

    fn tick(&self) -> Result<(), PlayError> {
        self.runtime.with_open_result(PlayerRuntime::tick)
    }

    fn playback_view(&self) -> PlaybackView {
        if self.runtime.is_closed() {
            return PlaybackView::default();
        }
        self.runtime
            .playback_snapshot()
            .map(PlaybackView::from)
            .unwrap_or_default()
    }

    fn set_host_level(&self, level: f32) {
        if !self.runtime.is_closed() {
            self.runtime.core.engine.commit_desired_master_volume(level);
        }
    }

    fn host_level(&self) -> f32 {
        self.runtime.core.engine.master_volume()
    }

    fn close(&mut self) -> Result<(), PlayError> {
        self.make_control().close()
    }
}

impl PlayerControlSource for PlayerImpl {
    type Control = crate::player::PlayerControl;

    fn control(&self) -> Self::Control {
        self.make_control()
    }

    fn attach_session(&mut self, binding: SessionBinding) -> Result<(), PlayError> {
        self.runtime.attach_session(binding)
    }

    fn close_control(control: &Self::Control) -> Result<(), PlayError> {
        control.close()
    }

    #[cfg(target_arch = "wasm32")]
    fn take_host_member(&mut self) -> Result<PlayerMember, PlayError> {
        let sync = self.sync.take().ok_or_else(|| {
            PlayError::Internal("player synchronization ownership was already transferred".into())
        })?;
        Ok(PlayerMember::new(
            sync,
            self.runtime.core.engine.master_volume(),
        ))
    }
}
