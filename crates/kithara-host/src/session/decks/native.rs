use std::task::{Wake, Waker};

use kithara_platform::sync::{Arc, Weak, mpsc};
use kithara_play::PlayError;
use kithara_warp::BeatGridId;
use tracing::warn;

use super::{Deck, Decks};

impl Decks {
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Runs one message from the Host on the session thread that holds the
    /// decks.
    pub(crate) fn run(&mut self, message: DeckMsg) {
        match message {
            DeckMsg::Hold(id, deck) => self.hold(id, deck),
            DeckMsg::Close(id, reply_tx) => {
                if reply_tx.send(self.close(id)).is_err() {
                    warn!(?id, "host deck close receiver dropped");
                }
            }
            DeckMsg::Release(id, reply_tx) => {
                if reply_tx.send(self.release(id)).is_err() {
                    warn!(?id, "host deck release receiver dropped");
                }
            }
            DeckMsg::Drain(id) => self.drain(id),
            #[cfg(feature = "offline")]
            DeckMsg::Tick => self.tick(),
        }
    }
}

/// What a Host asks of the session thread that holds its decks.
pub(crate) enum DeckMsg {
    Hold(BeatGridId, Deck),
    Close(BeatGridId, mpsc::Sender<Result<(), PlayError>>),
    Release(BeatGridId, mpsc::Sender<Result<Deck, PlayError>>),
    /// The deck has commands posted: run them now, between ticks.
    Drain(BeatGridId),
    /// Tick every deck ahead of the next block an offline Host renders.
    #[cfg(feature = "offline")]
    Tick,
}

/// The session thread that holds a Host's decks, as the Host reaches it.
pub(crate) trait DeckInbox: Send + Sync + 'static {
    /// Posts `message` to the session thread and wakes it.
    ///
    /// # Errors
    /// Returns [`PlayError::SessionGone`] once the session thread stopped.
    fn post(&self, message: DeckMsg) -> Result<(), PlayError>;
}

/// A Host's way to the decks its session thread holds.
pub(crate) struct SessionDecks(Arc<dyn DeckInbox>);

impl SessionDecks {
    pub(crate) fn new(inbox: Arc<dyn DeckInbox>) -> Self {
        Self(inbox)
    }

    /// Hands `deck` to the session thread, which drains it once it holds it:
    /// a command posted before then woke no one. The deck is held before it
    /// leaves, so a command posted as soon as this returns reaches it.
    pub(crate) fn hold(&self, id: BeatGridId, mut deck: Deck) -> Result<(), PlayError> {
        deck.hold(Waker::from(Arc::new(DeckWake {
            id,
            inbox: Arc::downgrade(&self.0),
        })));
        self.0.post(DeckMsg::Hold(id, deck))
    }

    /// Closes the deck `id` on the session thread that holds it; a failed
    /// close leaves it held.
    pub(crate) fn close(&self, id: BeatGridId) -> Result<(), PlayError> {
        self.ask(|reply_tx| DeckMsg::Close(id, reply_tx))
    }

    /// Takes the deck `id` back from the session thread, released.
    pub(crate) fn release(&self, id: BeatGridId) -> Result<Deck, PlayError> {
        self.ask(|reply_tx| DeckMsg::Release(id, reply_tx))
    }

    /// Posts the message `message` builds around its reply and waits for the
    /// session thread's answer.
    fn ask<T>(
        &self,
        message: impl FnOnce(mpsc::Sender<Result<T, PlayError>>) -> DeckMsg,
    ) -> Result<T, PlayError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.0.post(message(reply_tx))?;
        reply_rx.recv().map_err(|_| PlayError::SessionGone {
            reason: "session thread stopped before answering for a deck",
        })?
    }

    /// Ticks the decks ahead of the block an offline Host renders next: the
    /// session thread runs what it is sent in order, so the tick lands
    /// before that block.
    #[cfg(feature = "offline")]
    pub(crate) fn tick_block(&self) {
        if self.0.post(DeckMsg::Tick).is_err() {
            warn!("offline session stopped before ticking a block");
        }
    }
}

/// Wakes the session thread to drain one deck it holds.
struct DeckWake {
    id: BeatGridId,
    inbox: Weak<dyn DeckInbox>,
}

impl Wake for DeckWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    /// A session the Host let go of released its decks as it stopped, so a
    /// wake that finds it gone had nothing left to run.
    fn wake_by_ref(self: &Arc<Self>) {
        if let Some(inbox) = self.inbox.upgrade() {
            drop(inbox.post(DeckMsg::Drain(self.id)));
        }
    }
}
