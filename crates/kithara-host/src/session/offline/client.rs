use kithara_audio::ConsumerWakeMode;
use kithara_bufpool::SampleBuffer;
use kithara_command::Ticket;
use kithara_platform::sync::{Mutex, mpsc};
use kithara_play::PlayError;
use kithara_worker::TaskControl;

use super::{OfflineSessionError, task::OfflineMsg};
#[cfg(not(target_arch = "wasm32"))]
use crate::session::decks::{DeckInbox, DeckMsg};
use crate::session::{
    HostCmd, HostDispatcher,
    protocol::{HostDispatchError, HostPostbox, not_taken},
};

pub(crate) struct OfflineSessionClient<S> {
    postbox: HostPostbox<S>,
    cmd_tx: Mutex<mpsc::Sender<OfflineMsg>>,
    control: TaskControl,
}

impl<S> OfflineSessionClient<S> {
    pub(super) fn new(
        postbox: HostPostbox<S>,
        cmd_tx: mpsc::Sender<OfflineMsg>,
        control: TaskControl,
    ) -> Self {
        Self {
            control,
            postbox,
            cmd_tx: Mutex::new(cmd_tx),
        }
    }

    pub(crate) fn position(&self) -> Result<u64, OfflineSessionError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.send(OfflineMsg::Position { reply_tx })
            .map_err(|_| OfflineSessionError::SessionGone)?;
        reply_rx
            .recv()
            .map_err(|_| OfflineSessionError::SessionGone)
    }

    pub(crate) fn render(
        &self,
        position: u64,
        frames: u32,
    ) -> Result<SampleBuffer, OfflineSessionError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.send(OfflineMsg::Render {
            position,
            frames,
            reply_tx,
        })
        .map_err(|_| OfflineSessionError::SessionGone)?;
        reply_rx
            .recv()
            .map_err(|_| OfflineSessionError::SessionGone)?
    }

    fn send(&self, message: OfflineMsg) -> Result<(), Box<OfflineMsg>> {
        self.cmd_tx
            .lock()
            .send(message)
            .map_err(|error| Box::new(error.0))?;
        self.control.wake();
        Ok(())
    }
}

impl<S: Send + Sync + 'static> HostDispatcher<S> for OfflineSessionClient<S> {
    /// Offline render pulls the graph from the session task, an ordinary thread
    /// that may block and read the clock, so a reader wakes its producer inline.
    fn consumer_wake_mode(&self) -> ConsumerWakeMode {
        ConsumerWakeMode::ImmediateOffRt
    }

    /// Posts `cmd`, then tells the session task, so the post keeps its order
    /// with the messages sent around it.
    fn dispatch(&self, cmd: HostCmd<S>) -> Result<Ticket<PlayError>, HostDispatchError> {
        let ticket = self.postbox.post(cmd).map_err(not_taken)?;
        self.send(OfflineMsg::Posted).map_err(|_| {
            HostDispatchError::NotTaken(PlayError::SessionGone {
                reason: "offline session stopped accepting commands",
            })
        })?;
        Ok(ticket)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl<S: Send + Sync + 'static> DeckInbox for OfflineSessionClient<S> {
    fn post(&self, message: DeckMsg) -> Result<(), PlayError> {
        self.send(OfflineMsg::Deck(message))
            .map_err(|_| PlayError::SessionGone {
                reason: "offline session stopped taking decks",
            })
    }
}
