use std::iter;

use ringbuf::{
    HeapCons, HeapProd, HeapRb,
    traits::{Consumer, Producer, Split},
};

use super::Inbox;
use crate::{
    config::ChannelConfig,
    protocol::{Batch, Protocol, Seq, Target, When},
    receipt::Receipt,
};

/// Why a batch was not sent; the batch comes back whole.
#[derive(Debug, thiserror::Error)]
pub enum SendError<P: Protocol> {
    /// The channel's capacity of batches is already in flight.
    #[error("the channel already holds its capacity of batches in flight")]
    Full(Batch<P>),
    /// The basis names a target the channel does not track.
    #[error("the batch basis names a target outside the channel")]
    Target(Batch<P>),
}

/// A batch on its way to the executor, numbered at send.
pub(super) struct Sent<P: Protocol> {
    pub(super) batch: Batch<P>,
    pub(super) seq: Seq,
    pub(super) when: When<P::Clock>,
}

impl<P: Protocol> Sent<P> {
    /// Execution order: earlier moments first, then send order.
    pub(super) fn key(&self) -> (When<P::Clock>, Seq) {
        (self.when, self.seq)
    }
}

/// Sending half of a channel, owned by one thread.
pub struct Sender<P: Protocol> {
    receipts: HeapCons<Receipt<P>>,
    commands: HeapProd<Sent<P>>,
    next: Seq,
    credits: usize,
    targets: usize,
}

impl<P: Protocol> Sender<P> {
    /// Receipts that arrived since the last call, in the order the executor
    /// answered them. Each one returns a credit to the sender.
    pub fn receipts(&mut self) -> impl Iterator<Item = Receipt<P>> {
        iter::from_fn(move || {
            let receipt = self.receipts.try_pop()?;
            self.credits += 1;
            Some(receipt)
        })
    }

    /// Sends `batch` to apply at `when` and returns its number.
    ///
    /// # Errors
    ///
    /// Returns the batch whole as [`SendError::Target`] when its basis names a
    /// target at or past the configured count, and as [`SendError::Full`] when
    /// the channel's capacity of batches is already in flight. Neither spends
    /// a number.
    pub fn send(&mut self, when: When<P::Clock>, batch: Batch<P>) -> Result<Seq, SendError<P>> {
        if batch
            .basis
            .iter()
            .any(|&(target, _)| target.index() >= self.targets)
        {
            return Err(SendError::Target(batch));
        }
        let Some(credits) = self.credits.checked_sub(1) else {
            return Err(SendError::Full(batch));
        };
        let seq = self.next;
        if let Err(sent) = self.commands.try_push(Sent { batch, seq, when }) {
            return Err(SendError::Full(sent.batch));
        }
        self.credits = credits;
        self.next = seq.next();
        Ok(seq)
    }
}

/// Builds a channel to one executor.
///
/// The sender starts with one credit per batch of capacity and gets a credit
/// back with each receipt, so the executor's schedule and its receipt ring
/// never overflow.
#[must_use]
pub fn channel<P: Protocol>(config: ChannelConfig) -> (Sender<P>, Inbox<P>) {
    let capacity = config.capacity.get();
    let (commands, pending) = HeapRb::<Sent<P>>::new(capacity).split();
    let (answers, receipts) = HeapRb::<Receipt<P>>::new(capacity).split();
    let sender = Sender {
        commands,
        receipts,
        next: Seq::FIRST,
        credits: capacity,
        targets: config.targets,
    };
    (sender, Inbox::new(pending, answers, config))
}
