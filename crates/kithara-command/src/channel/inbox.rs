use std::{
    mem,
    ops::Range,
    task::{Context, Poll},
};

use futures::task::AtomicWaker;
use kithara_platform::sync::Arc;
use ringbuf::{
    HeapCons, HeapProd,
    traits::{Consumer, Observer, Producer},
};

use super::{gate::Gate, ledger::Ledger, schedule::Schedule, sender::Sent};
use crate::{
    config::ChannelConfig,
    protocol::{Batch, Protocol, Seq, When},
    receipt::{Outcome, Receipt, Rejection},
};

/// Receiving half of a channel, owned by the executor's thread.
///
/// Each block the executor calls [`Inbox::drain`], then takes batches through
/// [`Inbox::next_due`] and answers each through [`Due::apply`] or
/// [`Due::refuse`] before taking the next one. An executor whose work on a
/// batch outlasts the call parks it through [`Due::defer`] and answers it
/// later through [`Inbox::resume`]; one that waits for batches instead of
/// running blocks drains through [`Inbox::poll_drain`].
pub struct Inbox<P: Protocol> {
    pending: HeapCons<Sent<P>>,
    answers: HeapProd<Receipt<P>>,
    wake: Arc<AtomicWaker>,
    /// The sender's owner's waker, rung with each receipt.
    answered: Arc<AtomicWaker>,
    /// Closed as the inbox drops, so no batch is sent past its last drain.
    gate: Arc<Gate>,
    ledger: Ledger,
    schedule: Schedule<P>,
    parked: Vec<Parked<P>>,
}

/// A due batch its executor answers later.
struct Parked<P: Protocol> {
    at: P::Clock,
    seq: Seq,
    basis: Vec<(P::Target, Option<Seq>)>,
    commands: Vec<P::Command>,
    offset: usize,
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
    /// What the receipt reports when the due batch drops; `None` once the
    /// batch is parked for a later answer.
    outcome: Option<Outcome<P>>,
    /// Number the sender gave the batch.
    #[field(get(copy))]
    seq: Seq,
    /// Targets the batch shifts, as judged.
    #[field(get)]
    basis: Vec<(P::Target, Option<Seq>)>,
    /// Commands to apply. The executor moves the resources it takes out of
    /// them, or swaps the ones it releases into them; the receipt returns
    /// what is left, with the vector's allocation, to the sender.
    #[field(get, get_mut(deref = false))]
    commands: Vec<P::Command>,
    /// Frames from the block start to the batch's moment.
    #[field(get)]
    offset: usize,
}

/// One stretch of a block an executor renders through [`Inbox::run_block`].
pub enum Step<'inbox, P: Protocol> {
    /// Frames of the block, from its start, that no due batch splits.
    Run(Range<usize>),
    /// A batch due at the frame the stretches before it ended on.
    Due(Due<'inbox, P>),
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
        wake: Arc<AtomicWaker>,
        answered: Arc<AtomicWaker>,
        gate: Arc<Gate>,
        config: ChannelConfig,
    ) -> Self {
        Self {
            pending,
            answers,
            wake,
            answered,
            gate,
            schedule: Schedule::new(config.capacity.get()),
            parked: Vec::with_capacity(config.capacity.get()),
            ledger: Ledger::new(config.targets),
        }
    }

    /// Moves every batch sent since the last call into the schedule.
    pub fn drain(&mut self) {
        while let Some(sent) = self.pending.try_pop() {
            self.schedule.insert(sent);
        }
    }

    /// Drains as [`Inbox::drain`] does, for an executor that waits for
    /// batches: `Ready` once a batch waits in the schedule or the sender is
    /// gone; otherwise the next send, or the sender's drop, wakes the task
    /// behind `cx`.
    pub fn poll_drain(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        self.wake.register(cx.waker());
        let closed = self.is_closed();
        self.drain();
        if closed || self.schedule.peek().is_some() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }

    /// Whether the sender is gone: no batch arrives after the ones already
    /// sent.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        !self.pending.write_is_held()
    }

    /// The batch parked under `seq` through [`Due::defer`], due again to
    /// answer now at the moment and offset it was due at; `None` when no
    /// batch is parked under that number.
    pub fn resume(&mut self, seq: Seq) -> Option<Due<'_, P>> {
        let index = self.parked.iter().position(|parked| parked.seq == seq)?;
        let Parked {
            at,
            seq,
            basis,
            commands,
            offset,
        } = self.parked.swap_remove(index);
        Some(Due {
            at,
            seq,
            basis,
            commands,
            offset,
            inbox: self,
            outcome: Some(Outcome::Rejected(Rejection::Unanswered)),
        })
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
                        outcome: Some(Outcome::Rejected(Rejection::Unanswered)),
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

    /// Walks the block of `frames` frames starting at `start` in frame order,
    /// for an executor that renders between batches: each batch due inside
    /// the block comes to `step` at the frame it applies on, after the
    /// stretch of frames before it and before the stretch after it. Batches
    /// that come late or stale are answered on the way, as by
    /// [`Inbox::next_due`].
    pub fn run_block<F>(&mut self, start: P::Clock, frames: usize, mut step: F)
    where
        F: FnMut(Step<'_, P>),
    {
        self.drain();
        let mut reached = 0;
        while let Some(due) = self.next_due(start, frames) {
            let offset = due.offset;
            if offset > reached {
                step(Step::Run(reached..offset));
                reached = offset;
            }
            step(Step::Due(due));
        }
        if reached < frames {
            step(Step::Run(reached..frames));
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
        self.answered.wake();
    }
}

impl<P: Protocol> Drop for Inbox<P> {
    /// Answers [`Rejection::Unanswered`] for every batch the inbox still
    /// holds, parked, scheduled or not yet drained, and returns each whole,
    /// so a sender never waits on an executor that is gone. It closes the
    /// gate first: a later send comes back closed instead of landing past
    /// the last drain.
    fn drop(&mut self) {
        self.gate.close();
        self.drain();
        while let Some(parked) = self.parked.pop() {
            self.reply(Receipt {
                seq: parked.seq,
                outcome: Outcome::Rejected(Rejection::Unanswered),
                batch: Batch {
                    basis: parked.basis,
                    commands: parked.commands,
                },
            });
        }
        while let Some(sent) = self.schedule.pop() {
            self.reply(Receipt {
                seq: sent.seq,
                outcome: Outcome::Rejected(Rejection::Unanswered),
                batch: sent.batch,
            });
        }
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
        self.outcome = Some(Outcome::Applied { data, at: self.at });
    }

    /// Refuses the batch for a reason of the executor's domain; its targets
    /// keep their last shift.
    pub fn refuse(mut self, refusal: P::Refusal) {
        self.outcome = Some(Outcome::Rejected(Rejection::Refused(refusal)));
    }

    /// Parks the batch in its inbox for an answer after this call, and
    /// returns its number for [`Inbox::resume`]. The batch keeps its credit
    /// until answered.
    pub fn defer(mut self) -> Seq {
        self.outcome = None;
        let parked = Parked {
            at: self.at,
            seq: self.seq,
            basis: mem::take(&mut self.basis),
            commands: mem::take(&mut self.commands),
            offset: self.offset,
        };
        debug_assert!(
            self.inbox.parked.len() < self.inbox.parked.capacity(),
            "credits bound the batches in flight"
        );
        self.inbox.parked.push(parked);
        self.seq
    }
}

impl<P: Protocol> Drop for Due<'_, P> {
    /// Answers the batch with its outcome and returns it whole; the empty
    /// vectors left in its place own no allocation. A parked batch answers
    /// when it is resumed.
    fn drop(&mut self) {
        let Some(outcome) = self.outcome.take() else {
            return;
        };
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
