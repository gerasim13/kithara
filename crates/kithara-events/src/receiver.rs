#![forbid(unsafe_code)]

use kithara_platform::tokio::sync::broadcast::{
    self,
    error::{RecvError, TryRecvError},
};

use crate::{Envelope, Event, EventBus, EventSet};

/// One event type's channel on one bus scope.
#[derive(derive_more::Debug)]
pub struct TopicReceiver<E: Event> {
    #[debug(skip)]
    rx: broadcast::Receiver<Envelope<E>>,
    closed: bool,
}

impl<E: Event> TopicReceiver<E> {
    /// Subscribes to `E` on `bus`, creating the channel if this is its first
    /// subscriber.
    #[must_use]
    pub fn new(bus: &EventBus) -> Self {
        Self {
            rx: bus.subscribe_topic::<E>(),
            closed: false,
        }
    }

    /// Waits for the next event.
    ///
    /// # Errors
    ///
    /// Returns `Lagged(n)` when the receiver fell `n` events behind and those
    /// events are gone, and `Closed` when the owning scope is gone.
    pub async fn recv(&mut self) -> Result<Envelope<E>, RecvError> {
        let received = self.rx.recv().await;
        if matches!(received, Err(RecvError::Closed)) {
            self.closed = true;
        }
        received
    }

    /// Takes the next event if one is already queued.
    ///
    /// # Errors
    ///
    /// Returns `Empty` when nothing is queued, `Lagged(n)` when the receiver
    /// fell `n` events behind, and `Closed` when the owning scope is gone.
    pub fn try_recv(&mut self) -> Result<Envelope<E>, TryRecvError> {
        let received = self.rx.try_recv();
        if matches!(received, Err(TryRecvError::Closed)) {
            self.closed = true;
        }
        received
    }

    /// Whether this channel has reported `Closed`.
    ///
    /// `EventSet::recv` reads this to disable a dead branch of its `select!`
    /// instead of spinning on it.
    #[must_use]
    pub const fn is_closed(&self) -> bool {
        self.closed
    }
}

/// A receiver for one [`EventSet`] — one channel per member.
#[derive(derive_more::Debug)]
pub struct EventReceiver<S: EventSet> {
    #[debug(skip)]
    rx: S::Receivers,
}

impl<S: EventSet> EventReceiver<S> {
    pub(crate) fn new(rx: S::Receivers) -> Self {
        Self { rx }
    }

    /// Waits for the next event from any member of the set, preferring the
    /// earlier-declared member when several have one queued.
    ///
    /// # Errors
    /// Returns lag information or `Closed` when all members close.
    pub async fn recv(&mut self) -> Result<Envelope<S>, RecvError> {
        S::recv(&mut self.rx).await
    }

    /// Takes the next queued event from any member of the set, preferring the
    /// earlier-declared member when several have one queued.
    ///
    /// # Errors
    /// Returns `Empty`, lag information, or `Closed` when all members close.
    pub fn try_recv(&mut self) -> Result<Envelope<S>, TryRecvError> {
        S::try_recv(&mut self.rx)
    }
}
