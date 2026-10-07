use kithara_bufpool::SampleBuffer;
use kithara_command::Ticket;
use kithara_platform::sync::{Mutex, mpsc};
use kithara_play::PlayError;
use kithara_worker::TaskControl;

use super::{OfflineSessionError, task::OfflineMsg};
use crate::session::{
    decks::{DeckInbox, DeckMsg},
    protocol::{HostDispatchError, HostDispatcher, HostPostbox, not_taken},
};

pub(crate) struct OfflineSessionClient<C> {
    postbox: HostPostbox<C>,
    cmd_tx: Mutex<mpsc::Sender<OfflineMsg>>,
    control: TaskControl,
}

impl<C> OfflineSessionClient<C> {
    pub(super) fn new(
        postbox: HostPostbox<C>,
        cmd_tx: mpsc::Sender<OfflineMsg>,
        control: TaskControl,
    ) -> Self {
        Self {
            postbox,
            cmd_tx: Mutex::new(cmd_tx),
            control,
        }
    }
    pub(crate) fn position(&self) -> Result<u64, OfflineSessionError> {
        todo!("Read the rendered cursor from the offline owner's published snapshot (spec §4.1)")
    }
    pub(crate) fn render(
        &self,
        _position: u64,
        _frames: u32,
    ) -> Result<SampleBuffer, OfflineSessionError> {
        todo!(
            "Post one finite render request and wait for its output receipt off the owner thread; the generic owner has no render receipt payload yet (spec §4.1)"
        )
    }
    fn send(&self, message: OfflineMsg) -> Result<(), PlayError> {
        self.cmd_tx
            .lock()
            .send(message)
            .map_err(|_| PlayError::SessionGone {
                reason: "offline session stopped taking commands",
            })?;
        self.control.wake();
        Ok(())
    }
}

impl<C: Send + 'static> HostDispatcher<C> for OfflineSessionClient<C> {
    fn dispatch(&self, command: C) -> Result<Ticket<PlayError>, HostDispatchError> {
        let ticket = self.postbox.post(command).map_err(not_taken)?;
        self.send(OfflineMsg::Posted)
            .map_err(HostDispatchError::NotTaken)?;
        Ok(ticket)
    }
    fn shutdown(&self) {
        drop(self.send(OfflineMsg::Shutdown));
    }
}

impl<C: Send + 'static> DeckInbox for OfflineSessionClient<C> {
    fn post(&self, message: DeckMsg) -> Result<(), PlayError> {
        self.send(OfflineMsg::Deck(message))
    }
}
