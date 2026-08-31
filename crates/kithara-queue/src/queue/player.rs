use kithara_play::{
    BeatGrid, BeatGridId, BeatGridSnapshot, PlayError, SeekOutcome, SessionBinding, SyncAdmission,
    SyncApplied, SyncError, SyncGroup, SyncGroupSnapshot, SyncOperation, SyncRejected,
    SyncStatusSnapshot,
    player::{PlaybackView, Player, PlayerControlSource, PlayerMember},
};

use super::Queue;

impl BeatGrid for Queue {
    delegate::delegate! {
        to self.player {
            fn id(&self) -> BeatGridId;
            fn snapshot(&self) -> BeatGridSnapshot;
        }
    }
}

impl SyncGroup for Queue {
    type NestedGroup = PlayerMember;

    delegate::delegate! {
        to self.player {
            fn topology(&self) -> Result<SyncGroupSnapshot, SyncError>;

            fn transact(
                &mut self,
                operation: SyncOperation<PlayerMember>,
            ) -> Result<SyncAdmission, SyncRejected<PlayerMember>>;

            fn acknowledge(
                &mut self,
                applied: SyncApplied,
            ) -> Result<SyncStatusSnapshot, SyncError>;
        }
    }

    fn status(&self) -> SyncStatusSnapshot {
        SyncGroup::status(&self.player)
    }
}

impl Player for Queue {
    delegate::delegate! {
        to self.control {
            fn play(&self);
            fn pause(&self);
            fn playback_view(&self) -> PlaybackView;
            fn close(&mut self) -> Result<(), PlayError>;
        }
        to self {
            #[call(seek_player)]
            fn seek_seconds(&self, seconds: f64) -> Result<SeekOutcome, PlayError>;
            #[call(tick_player)]
            fn tick(&self) -> Result<(), PlayError>;
        }
        to self.player {
            fn set_host_level(&self, level: f32);
            fn host_level(&self) -> f32;
        }
    }
}

impl PlayerControlSource for Queue {
    type Control = super::QueueControl;

    fn control(&self) -> Self::Control {
        self.control.clone()
    }

    fn close_control(control: &Self::Control) -> Result<(), PlayError> {
        control.close()
    }

    delegate::delegate! {
        to self.player {
            fn attach_session(&mut self, binding: SessionBinding) -> Result<(), PlayError>;
            #[cfg(target_arch = "wasm32")]
            fn take_host_member(&mut self) -> Result<PlayerMember, PlayError>;
        }
    }
}
