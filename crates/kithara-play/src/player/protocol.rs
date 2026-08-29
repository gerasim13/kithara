use std::fmt;

use kithara_audio::SeekOutcome;
use kithara_warp::{
    BeatGrid, BeatGridId, BeatGridSnapshot, SyncAdmission, SyncApplied, SyncError, SyncGroup,
    SyncGroupSnapshot, SyncOperation, SyncRejected, SyncStatusSnapshot,
};

use super::{PlaybackView, PlayerImpl, PlayerRuntime};
use crate::{PlayError, SessionBinding};

/// Canonical object-safe protocol implemented by a standalone player and its
/// orchestration decorators.
///
/// Queue-specific item, EQ, volume, and event APIs remain on their concrete
/// facade. This contract contains only playback operations shared by every
/// host member plus the synchronization-group protocol.
pub trait Player: BeatGrid + SyncGroup<NestedGroup = PlayerMember> + Send + Sync + 'static {
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

    /// Stop owned work and detach the player from its playback session.
    fn close(&mut self) -> Result<(), PlayError>;
}

/// Produces a cloneable command capability without sharing player identity or
/// synchronization topology.
pub trait PlayerControlSource: Player {
    /// Concrete command capability retained by typed host-owned handles.
    type Control: Clone + Send + Sync + 'static;

    /// Creates a command capability for this player.
    fn control(&self) -> Self::Control;

    /// Attaches the resident Player to its canonical session exactly once.
    fn attach_session(&mut self, binding: SessionBinding) -> Result<(), PlayError>;

    /// Closes the resident player through a previously issued capability.
    fn close_control(control: &Self::Control) -> Result<(), PlayError>;
}

/// Exclusively owned, sized erasure of one concrete [`Player`].
///
/// Host dispatch uses closure-based access so a reference to the resident
/// player cannot escape the owner's command/lock boundary.
pub struct PlayerMember {
    inner: Box<dyn Player>,
}

impl PlayerMember {
    /// Erases one concrete player while retaining exclusive ownership.
    #[must_use]
    pub fn new<P: Player>(player: P) -> Self {
        Self {
            inner: Box::new(player),
        }
    }

    /// Dispatches against the object-safe player protocol without exposing a
    /// borrowed player in the result.
    pub fn dispatch<R, F>(&self, dispatch: F) -> R
    where
        R: 'static,
        F: for<'a> FnOnce(&'a dyn Player) -> R,
    {
        dispatch(self.inner.as_ref())
    }
}

impl fmt::Debug for PlayerMember {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PlayerMember")
            .field("grid_id", &self.id())
            .finish_non_exhaustive()
    }
}

impl BeatGrid for PlayerMember {
    delegate::delegate! {
        to self.inner.as_ref() {
            fn id(&self) -> BeatGridId;
            fn snapshot(&self) -> BeatGridSnapshot;
        }
    }
}

impl SyncGroup for PlayerMember {
    type NestedGroup = Self;

    delegate::delegate! {
        to self.inner.as_mut() {
            fn transact(
                &mut self,
                operation: SyncOperation<Self>,
            ) -> Result<SyncAdmission, SyncRejected<Self>>;
            fn acknowledge(&mut self, applied: SyncApplied) -> Result<SyncStatusSnapshot, SyncError>;
        }
    }

    fn topology(&self) -> Result<SyncGroupSnapshot, SyncError> {
        self.inner.topology()
    }

    fn status(&self) -> SyncStatusSnapshot {
        SyncGroup::status(self.inner.as_ref())
    }
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
}
