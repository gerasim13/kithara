use std::task::Waker;

use kithara_bufpool::HasPool;
use kithara_play::{
    BeatGridId, PlayError, SessionBinding,
    player::{Player, PlayerControlSource},
};

use super::Queue;

impl<S> Player for Queue<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    fn drain(&mut self) {}

    fn hold(&mut self, _waker: Waker) {}

    fn release(&mut self) {}

    delegate::delegate! {
        to self.control {
            fn close(&mut self) -> Result<(), PlayError>;
        }
        to self {
            #[call(tick_player)]
            fn tick(&mut self) -> Result<(), PlayError>;
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
            ) -> Result<BeatGridId, PlayError>;
        }
    }
}
