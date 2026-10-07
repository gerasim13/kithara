use std::task::Waker;

use kithara_bufpool::HasPool;
use kithara_platform::sync::Arc;
use kithara_play::{
    AllocatedSlot, DeckRegistration, PlayError, SessionBinding,
    player::{Player, PlayerControlSource},
};

use super::{Queue, QueueControl};

impl<S> Player for Queue<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    /// Closes the resident player, then irreversibly cancels queue-owned
    /// work. A failed close leaves the queue open, so its holder can retry.
    fn close(&mut self) -> Result<(), PlayError> {
        self.player.close()?;
        self.shutdown.cancel();
        Ok(())
    }

    delegate::delegate! {
        to self {
            #[call(drain_commands)]
            fn drain(&mut self);
            #[call(tick_player)]
            fn tick(&mut self) -> Result<(), PlayError>;
        }
        to self.mailbox {
            fn hold(&mut self, waker: Waker);
            fn release(&mut self);
        }
    }
}

impl<S> PlayerControlSource for Queue<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    type Control = QueueControl<S>;
    type Schema = S;

    fn close_control(control: &Self::Control) -> Result<(), PlayError> {
        control.close()
    }

    fn control(&self) -> Self::Control {
        QueueControl {
            postbox: self.postbox.clone(),
            runtime: Arc::clone(&self.runtime),
        }
    }

    delegate::delegate! {
        to self.resident {
            fn attach_session(
                &mut self,
                binding: SessionBinding<S>,
            ) -> Result<DeckRegistration<S>, PlayError>;
            fn seat(&mut self, slot: AllocatedSlot);
        }
    }
}
