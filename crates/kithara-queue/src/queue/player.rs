use kithara_bufpool::HasPool;
use kithara_play::{
    BeatGrid, BeatGridId, BeatGridSnapshot, PlayError, SeekOutcome, SessionBinding, SyncAdmission,
    SyncApplied, SyncError, SyncGroup, SyncGroupSnapshot, SyncOperation, SyncRejected,
    SyncStatusSnapshot,
    player::{PlaybackView, Player, PlayerControlSource, PlayerMember},
};

use super::Queue;

impl<S> BeatGrid for Queue<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    delegate::delegate! {
        to self.player {
            fn id(&self) -> BeatGridId;
            fn snapshot(&self) -> BeatGridSnapshot;
        }
    }
}

impl<S> SyncGroup for Queue<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    type NestedGroup = PlayerMember;

    fn status(&self) -> SyncStatusSnapshot {
        SyncGroup::status(&self.player)
    }

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
}

impl<S> Player for Queue<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
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

impl<S> PlayerControlSource for Queue<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    type Control = super::QueueControl<S>;
    type Schema = S;

    fn close_control(control: &Self::Control) -> Result<(), PlayError> {
        control.close()
    }

    fn control(&self) -> Self::Control {
        self.control.clone()
    }

    delegate::delegate! {
        to self.player {
            fn attach_session(&mut self, binding: SessionBinding<S>) -> Result<(), PlayError>;
            #[cfg(target_arch = "wasm32")]
            fn take_host_member(&mut self) -> Result<PlayerMember, PlayError>;
        }
    }
}
