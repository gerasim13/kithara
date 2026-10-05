use std::mem;

use ringbuf::{
    HeapCons, HeapProd,
    traits::{Consumer, Producer},
};

use super::{ledger::Ledger, schedule::Schedule, sender::Sent};
use crate::{
    config::ChannelConfig,
    protocol::{Batch, Protocol, Seq, When},
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
    /// The batch's moment; the block start for a batch sent for the next block.
    #[field(get(copy))]
    at: P::Clock,
    /// What the receipt reports when the due batch drops.
    outcome: Outcome<P>,
    /// Number the sender gave the batch.
    #[field(get(copy))]
    seq: Seq,
    /// Targets the batch shifts, as judged.
    #[field(get)]
    basis: Vec<(P::Target, Option<Seq>)>,
    /// Commands to apply; the executor swaps the resources it releases into
    /// them.
    #[field(get, get_mut)]
    commands: Vec<P::Command>,
    /// Frames from the block start to the batch's moment.
    #[field(get)]
    offset: usize,
}

/// Where a moment falls against a block it does not come after.
enum Place<P: Protocol> {
    Past,
    Within { offset: usize, at: P::Clock },
}

impl<P: Protocol> Place<P> {
    /// `None` when the block of `frames` frames is empty or the moment comes
    /// after it.
    fn of(when: When<P::Clock>, start: P::Clock, frames: usize) -> Option<Self> {
        if frames == 0 {
            return None;
        }
        let When::At(at) = when else {
            return Some(Self::Within {
                offset: 0,
                at: start,
            });
        };
        match P::frames_since(at, start).map(usize::try_from) {
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

    /// Frames from `start` to the moment of the earliest waiting batch, for an
    /// executor that ends its block there; zero when that batch waits for the
    /// next block or its moment is not after `start`. `None` when nothing
    /// waits.
    #[must_use]
    pub fn frames_until_due(&self, start: P::Clock) -> Option<u64> {
        Some(match self.schedule.peek()? {
            When::Next => 0,
            When::At(at) => P::frames_since(at, start).unwrap_or(0),
        })
    }

    /// Next batch due in the block of `frames` frames starting at `start`.
    ///
    /// Batches come in time order and, at one moment, in send order. On the
    /// way it answers [`Rejection::Late`] for a batch whose moment is before
    /// `start` and [`Rejection::Stale`] for one whose basis no longer matches.
    /// `None` means nothing more is due inside the block.
    pub fn next_due(&mut self, start: P::Clock, frames: usize) -> Option<Due<'_, P>> {
        loop {
            let place = Place::<P>::of(self.schedule.peek()?, start, frames)?;
            let sent = self.schedule.pop()?;
            let rejection = match place {
                Place::Within { offset, at } if self.ledger.is_current(&sent.batch.basis) => {
                    let Batch { basis, commands } = sent.batch;
                    return Some(Due {
                        offset,
                        at,
                        basis,
                        commands,
                        inbox: self,
                        seq: sent.seq,
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

    /// Refuses with `refusal`, in time order, every batch waiting for a
    /// moment of the clock, for an executor whose clock starts a new axis on
    /// which those moments no longer fall. A batch for the next block keeps
    /// waiting.
    pub fn refuse_timed(&mut self, refusal: P::Refusal)
    where
        P::Refusal: Clone,
    {
        self.drain();
        while let Some(sent) = self.schedule.take_timed() {
            self.reply(Receipt {
                seq: sent.seq,
                outcome: Outcome::Rejected(Rejection::Refused(refusal.clone())),
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
    /// Applies the batch: the targets of its basis, which stays as judged,
    /// record it as their last shift, and its receipt reports `data` at the
    /// batch's moment.
    /// ```compile_fail
    /// # use kithara_command::{Due, Protocol};
    /// fn move_basis<P: Protocol>(due: &mut Due<'_, P>) {
    ///     due.basis_mut()[0].1 = None; // ERROR: the basis is read-only
    /// }
    /// ```
    pub fn apply(mut self, data: P::Applied) {
        self.inbox.ledger.record(&self.basis, self.seq);
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
    /// vectors left in its place own no allocation.
    fn drop(&mut self) {
        let outcome = mem::replace(&mut self.outcome, Outcome::Rejected(Rejection::Unanswered));
        let batch = Batch {
            basis: mem::take(&mut self.basis),
            commands: mem::take(&mut self.commands),
        };
        self.inbox.reply(Receipt {
            outcome,
            batch,
            seq: self.seq,
        });
    }
}
