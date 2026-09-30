use ringbuf::{
    HeapCons, HeapProd,
    traits::{Consumer, Producer},
};

use super::{schedule::Schedule, sender::Sent};
use crate::{
    config::ChannelConfig,
    protocol::{Batch, Clock, Protocol, Seq, When},
    receipt::{Outcome, Receipt, Rejection},
};

/// Receiving half of a channel, owned by the executor's thread.
///
/// Each block the executor calls [`Inbox::drain`], then takes batches through
/// [`Inbox::next_due`] and answers each through [`Inbox::apply`] or
/// [`Inbox::refuse`] before taking the next one.
pub struct Inbox<P: Protocol> {
    pending: HeapCons<Sent<P>>,
    answers: HeapProd<Receipt<P>>,
    schedule: Schedule<P>,
}

/// A batch due inside the current block.
#[derive(Debug, fieldwork::Fieldwork)]
#[fieldwork(opt_in, get, get_mut)]
#[must_use = "a due batch is answered through Inbox::apply or Inbox::refuse"]
pub struct Due<P: Protocol> {
    /// Commands to apply; resources the executor releases go back into it.
    #[field(get, get_mut)]
    batch: Batch<P>,
    /// The batch's moment; the block start for a batch sent for the next block.
    #[field(get(copy))]
    at: P::Clock,
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
        }
    }

    /// Applies `due`: its receipt reports `data` at the batch's moment.
    pub fn apply(&mut self, due: Due<P>, data: P::Applied) {
        self.reply(Receipt {
            seq: due.seq,
            outcome: Outcome::Applied { data, at: due.at },
            batch: due.batch,
        });
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
    /// `start`. `None` means nothing more is due inside the block.
    pub fn next_due(&mut self, start: P::Clock, frames: usize) -> Option<Due<P>> {
        loop {
            let place = Place::of(self.schedule.peek()?, start, frames)?;
            let sent = self.schedule.pop()?;
            let rejection = match place {
                Place::Within { offset, at } => {
                    return Some(Due {
                        offset,
                        at,
                        seq: sent.seq,
                        batch: sent.batch,
                    });
                }
                Place::Past => Rejection::Late,
            };
            self.reply(Receipt {
                seq: sent.seq,
                outcome: Outcome::Rejected(rejection),
                batch: sent.batch,
            });
        }
    }

    /// Refuses `due` for a reason of the executor's domain.
    pub fn refuse(&mut self, due: Due<P>, refusal: P::Refusal) {
        self.reply(Receipt {
            seq: due.seq,
            outcome: Outcome::Rejected(Rejection::Refused(refusal)),
            batch: due.batch,
        });
    }

    fn reply(&mut self, receipt: Receipt<P>) {
        let pushed = self.answers.try_push(receipt);
        debug_assert!(pushed.is_ok(), "credits bound the receipts in flight");
    }
}
