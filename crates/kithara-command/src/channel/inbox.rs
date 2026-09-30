use std::mem;

use ringbuf::{
    HeapCons, HeapProd,
    traits::{Consumer, Producer},
};

use super::{ledger::Ledger, schedule::Schedule, sender::Sent};
use crate::{
    config::ChannelConfig,
    protocol::{Batch, Clock, Protocol, Seq, When},
    receipt::{Outcome, Receipt, Rejection},
};

/// Receiving half of a channel, owned by the executor's thread.
///
/// Each block the executor calls [`Inbox::drain`], then takes batches through
/// [`Inbox::next_due`] and answers each through [`Due::apply`] or
/// [`Due::refuse`] before taking the next one.
pub struct Inbox<P: Protocol> {
    pending: HeapCons<Sent<P>>,
    answers: HeapProd<Receipt<P>>,
    ledger: Ledger,
    schedule: Schedule<P>,
}

/// A batch due inside the current block whose basis matches.
///
/// It holds its inbox until answered; dropped unanswered, it answers
/// [`Rejection::Unanswered`].
/// ```compile_fail
/// # use kithara_command::{Inbox, Protocol};
/// fn take_two<P: Protocol>(inbox: &mut Inbox<P>, at: P::Clock) {
///     let first = inbox.next_due(at, 64);
///     let second = inbox.next_due(at, 64); // ERROR: `first` still holds the inbox
///     drop((first, second));
/// }
/// ```
#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get, get_mut)]
#[must_use = "a due batch is answered through Due::apply or Due::refuse"]
pub struct Due<'inbox, P: Protocol> {
    inbox: &'inbox mut Inbox<P>,
    /// Commands to apply; resources the executor releases go back into it.
    #[field(get, get_mut)]
    batch: Batch<P>,
    /// The batch's moment; the block start for a batch sent for the next block.
    #[field(get(copy))]
    at: P::Clock,
    /// What the receipt reports when the due batch drops.
    outcome: Outcome<P>,
    seq: Seq,
    /// Frames from the block start to the batch's moment.
    #[field(get)]
    offset: usize,
}

/// Where a moment falls against a block it does not come after.
enum Place<T> {
    Past,
    Within { offset: usize, at: T },
}

impl<T: Clock> Place<T> {
    /// `None` when the moment comes after the block of `frames` frames.
    fn of(when: When<T>, start: T, frames: usize) -> Option<Self> {
        let When::At(at) = when else {
            return (frames > 0).then_some(Self::Within {
                offset: 0,
                at: start,
            });
        };
        match at.frames_since(start).map(usize::try_from) {
            None => Some(Self::Past),
            Some(Ok(offset)) if offset < frames => Some(Self::Within { offset, at }),
            Some(_) => None,
        }
    }
}

impl<P: Protocol> Inbox<P> {
    pub(super) fn new(
        pending: HeapCons<Sent<P>>,
        answers: HeapProd<Receipt<P>>,
        config: ChannelConfig,
    ) -> Self {
        Self {
            pending,
            answers,
            schedule: Schedule::new(config.capacity.get()),
            ledger: Ledger::new(config.targets),
        }
    }

    /// Moves every batch sent since the last call into the schedule.
    pub fn drain(&mut self) {
        while let Some(sent) = self.pending.try_pop() {
            self.schedule.insert(sent);
        }
    }

    /// Next batch due in the block of `frames` frames starting at `start`.
    ///
    /// Batches come in time order and, at one moment, in send order. On the
    /// way it answers [`Rejection::Late`] for a batch whose moment is before
    /// `start` and [`Rejection::Stale`] for one whose basis no longer matches.
    /// `None` means nothing more is due inside the block.
    pub fn next_due(&mut self, start: P::Clock, frames: usize) -> Option<Due<'_, P>> {
        loop {
            let place = Place::of(self.schedule.peek()?, start, frames)?;
            let sent = self.schedule.pop()?;
            let rejection = match place {
                Place::Within { offset, at } if self.ledger.is_current(&sent.batch.basis) => {
                    return Some(Due {
                        offset,
                        at,
                        inbox: self,
                        seq: sent.seq,
                        batch: sent.batch,
                        outcome: Outcome::Rejected(Rejection::Unanswered),
                    });
                }
                Place::Within { .. } => Rejection::Stale,
                Place::Past => Rejection::Late,
            };
            self.reply(Receipt {
                seq: sent.seq,
                outcome: Outcome::Rejected(rejection),
                batch: sent.batch,
            });
        }
    }

    fn reply(&mut self, receipt: Receipt<P>) {
        let pushed = self.answers.try_push(receipt);
        debug_assert!(pushed.is_ok(), "credits bound the receipts in flight");
    }
}

impl<P: Protocol> Due<'_, P> {
    /// Applies the batch: its targets record it as their last shift, and its
    /// receipt reports `data` at the batch's moment.
    pub fn apply(mut self, data: P::Applied) {
        self.inbox.ledger.record(&self.batch.basis, self.seq);
        self.outcome = Outcome::Applied { data, at: self.at };
    }

    /// Refuses the batch for a reason of the executor's domain; its targets
    /// keep their last shift.
    pub fn refuse(mut self, refusal: P::Refusal) {
        self.outcome = Outcome::Rejected(Rejection::Refused(refusal));
    }
}

impl<P: Protocol> Drop for Due<'_, P> {
    /// Answers the batch with its outcome and returns it whole; the empty
    /// batch left in its place owns no allocation.
    fn drop(&mut self) {
        let outcome = mem::replace(&mut self.outcome, Outcome::Rejected(Rejection::Unanswered));
        let batch = mem::replace(
            &mut self.batch,
            Batch {
                basis: Vec::new(),
                commands: Vec::new(),
            },
        );
        self.inbox.reply(Receipt {
            outcome,
            batch,
            seq: self.seq,
        });
    }
}
