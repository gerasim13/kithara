use kithara_bufpool::HasPool;
use kithara_play::{
    PlayError, SeekOutcome, SessionBinding,
    player::{PlaybackView, Player, PlayerControlSource},
};
use kithara_sync::SyncAttachment;

use super::Queue;

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

    fn prepare_control(control: &Self::Control) -> Result<(), PlayError> {
        control.with_open_result(|queue| queue.player.prepare())
    }

    delegate::delegate! {
        to self.player {
            fn attach_session(
                &mut self,
                binding: SessionBinding<S>,
            ) -> Result<SyncAttachment, PlayError>;
        }
    }
}
