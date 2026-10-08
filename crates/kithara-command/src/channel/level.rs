use std::mem;

use futures::task::AtomicWaker;

use super::{docket::Docket, scoped::Lifecycle, sender::Sent, sink::Sink};
use crate::{Batch, Outcome, Protocol, Receipt, Rejection, Seq, When};

/// An executor's view of one command level, with its own ledger and capacity.
pub struct LevelInbox<'inbox, P: Protocol> {
    pub(super) docket: &'inbox mut Docket<P>,
    pub(super) sink: Sink<'inbox, P>,
    pub(super) answered: &'inbox AtomicWaker,
    pub(super) lifecycle: Option<&'inbox mut Lifecycle>,
}

/// A batch due inside the block whose basis matches the level's ledger.
/// ```compile_fail
/// # use kithara_command::{Inbox, Protocol};
/// fn take_two<P: Protocol>(inbox: &mut Inbox<P>, at: P::Clock) {
///     let first = inbox.next_due(at, 64);
///     let second = inbox.next_due(at, 64);
///     drop((first, second));
/// }
/// ```
#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get, get_mut)]
#[must_use = "answer through Due::apply, Due::refuse, Due::defer or Due::commit"]
pub struct Due<'inbox, P: Protocol> {
    inbox: LevelInbox<'inbox, P>,
    #[field(get(copy))]
    at: P::Clock,
    outcome: Option<Outcome<P>>,
    #[field(get(copy))]
    seq: Seq,
    #[field(get)]
    basis: Vec<(P::Target, Option<Seq>)>,
    #[field(get, get_mut(deref = false))]
    commands: Vec<P::Command>,
    #[field(get)]
    offset: usize,
}

/// A deferred arrival, not judged until it is parked or resumed.
#[must_use = "park or refuse the deferred batch"]
pub struct Deferred<'inbox, P: Protocol> {
    inbox: LevelInbox<'inbox, P>,
    seq: Seq,
    basis: Vec<(P::Target, Option<Seq>)>,
    commands: Vec<P::Command>,
    outcome: Option<Outcome<P>>,
}

impl<'inbox, P: Protocol> LevelInbox<'inbox, P> {
    fn reborrow(&mut self) -> LevelInbox<'_, P> {
        LevelInbox {
            docket: self.docket,
            sink: self.sink.reborrow(),
            answered: self.answered,
            lifecycle: self.lifecycle.as_deref_mut(),
        }
    }

    /// Frames until the earliest Next or At batch; deferred and parked batches are excluded.
    #[must_use]
    pub fn frames_until_due(&self, start: P::Clock) -> Option<u64> {
        self.docket.frames_until_due(start)
    }

    /// Yields the next due batch, answering Late or Stale batches along the way.
    pub fn next_due(&mut self, start: P::Clock, frames: usize) -> Option<Due<'_, P>> {
        self.reborrow().take_due(start, frames)
    }

    pub(super) fn take_due(mut self, start: P::Clock, frames: usize) -> Option<Due<'inbox, P>> {
        if frames == 0 {
            return None;
        }
        loop {
            let (at, offset) = match self.docket.schedule.peek()? {
                When::Next => (start, Some(Ok(0))),
                When::At(at) => (at, P::frames_since(at, start).map(usize::try_from)),
                When::Deferred => return None,
            };
            let offset = match offset {
                Some(Ok(offset)) if offset < frames => Some(offset),
                Some(_) => return None,
                None => None,
            };
            let sent = self.docket.schedule.pop()?;
            let rejection = match offset {
                Some(offset) if self.docket.ledger.is_current(&sent.batch.basis) => {
                    return Some(self.due(sent, at, offset));
                }
                Some(_) => Rejection::Stale,
                None => Rejection::Late,
            };
            self.reject(sent, rejection);
        }
    }

    fn due(self, sent: Sent<P>, at: P::Clock, offset: usize) -> Due<'inbox, P> {
        Due {
            inbox: self,
            at,
            offset,
            seq: sent.seq,
            basis: sent.batch.basis,
            commands: sent.batch.commands,
            outcome: Some(Outcome::Rejected(Rejection::Unanswered)),
        }
    }

    /// Takes an arrival without judging its basis.
    pub fn next_deferred(&mut self) -> Option<Deferred<'_, P>> {
        self.reborrow().take_deferred()
    }

    pub(super) fn take_deferred(self) -> Option<Deferred<'inbox, P>> {
        if self.docket.arrived.is_empty() {
            return None;
        }
        let sent = self.docket.arrived.remove(0);
        Some(Deferred {
            inbox: self,
            seq: sent.seq,
            basis: sent.batch.basis,
            commands: sent.batch.commands,
            outcome: Some(Outcome::Rejected(Rejection::Unanswered)),
        })
    }

    /// Resumes a parked batch at `at`, judging it again and measuring its offset from `start`.
    pub fn resume(&mut self, seq: Seq, start: P::Clock, at: P::Clock) -> Option<Due<'_, P>> {
        self.reborrow().take_parked(seq, start, at)
    }

    pub(super) fn take_parked(
        mut self,
        seq: Seq,
        start: P::Clock,
        at: P::Clock,
    ) -> Option<Due<'inbox, P>> {
        let index = self.docket.parked.iter().position(|sent| sent.seq == seq)?;
        let offset = P::frames_since(at, start).and_then(|frames| usize::try_from(frames).ok());
        debug_assert!(
            offset.is_some(),
            "the firing moment must be inside the block"
        );
        let offset = offset?;
        let sent = self.docket.parked.swap_remove(index);
        if !self.docket.ledger.is_current(&sent.batch.basis) {
            self.reject(sent, Rejection::Stale);
            return None;
        }
        Some(self.due(sent, at, offset))
    }

    /// Whether this level still holds the parked batch.
    #[must_use]
    pub fn is_parked(&self, seq: Seq) -> bool {
        self.docket.is_parked(seq)
    }

    /// Edits a committed batch in place without taking it out of this level.
    pub fn committed_mut(&mut self, seq: Seq) -> Option<&mut [P::Command]> {
        self.docket.committed_mut(seq)
    }

    /// Answers a committed batch at its original moment, without judging its basis again.
    /// Returns false if this level no longer holds the committed batch.
    pub fn complete(&mut self, seq: Seq, data: P::Applied) -> bool {
        let Some(index) = self
            .docket
            .committed
            .iter()
            .position(|(_, sent)| sent.seq == seq)
        else {
            return false;
        };
        let (at, sent) = self.docket.committed.swap_remove(index);
        self.reply(Receipt {
            seq: sent.seq,
            batch: sent.batch,
            outcome: Outcome::Applied { at, data },
        });
        true
    }

    /// Whether this scope's Close has been drained; always false for the root.
    #[must_use]
    pub fn is_closing(&self) -> bool {
        self.lifecycle
            .as_ref()
            .is_some_and(|lifecycle| lifecycle.closing)
    }

    /// Returns leftovers whole, clears the ledger, advances generation and answers Closed last.
    ///
    /// # Panics
    /// Debug builds assert that only a closing scope is retired.
    pub fn retire(mut self) {
        debug_assert!(self.is_closing(), "only a closing scope can retire");
        if !self.is_closing() {
            return;
        }
        self.unanswered();
        self.docket.ledger.reset();
        if let Some(lifecycle) = self.lifecycle {
            lifecycle.generation = lifecycle.generation.wrapping_add(1);
            lifecycle.closing = false;
        }
        self.sink.closed(self.answered);
    }

    pub(super) fn refuse_timed(&mut self, refusal: P::Refusal)
    where
        P::Refusal: Clone,
    {
        while let Some(sent) = self.docket.schedule.take_timed() {
            self.reject(sent, Rejection::Refused(refusal.clone()));
        }
    }

    pub(super) fn unanswered(&mut self) {
        while let Some((_, sent)) = self.docket.committed.pop() {
            self.reject(sent, Rejection::Unanswered);
        }
        while let Some(sent) = self.docket.arrived.pop() {
            self.reject(sent, Rejection::Unanswered);
        }
        while let Some(sent) = self.docket.parked.pop() {
            self.reject(sent, Rejection::Unanswered);
        }
        while let Some(sent) = self.docket.schedule.pop() {
            self.reject(sent, Rejection::Unanswered);
        }
    }

    fn reject(&mut self, sent: Sent<P>, rejection: Rejection<P::Refusal>) {
        self.reply(Receipt {
            seq: sent.seq,
            batch: sent.batch,
            outcome: Outcome::Rejected(rejection),
        });
    }

    fn reply(&mut self, receipt: Receipt<P>) {
        self.sink.reply(receipt, self.answered);
    }

    fn reject_outdated(&mut self) {
        while let Some(sent) = self.docket.take_outdated() {
            self.reject(sent, Rejection::Stale);
        }
    }
}

impl<P: Protocol> Due<'_, P> {
    /// Records the batch and eagerly answers outdated batches now, retaining this batch
    /// and its credit until the executor completes it or the level returns its leftovers.
    pub fn commit(mut self) -> Seq {
        self.inbox.docket.ledger.record(&self.basis, self.seq);
        debug_assert!(
            self.inbox.docket.committed.len() < self.inbox.docket.committed.capacity(),
            "credits bound committed batches"
        );
        self.outcome = None;
        self.inbox.docket.committed.push((
            self.at,
            Sent {
                seq: self.seq,
                when: When::At(self.at),
                batch: Batch {
                    basis: mem::take(&mut self.basis),
                    commands: mem::take(&mut self.commands),
                },
            },
        ));
        self.inbox.reject_outdated();
        self.seq
    }

    /// Records every basis target and eagerly answers outdated batches of this level.
    /// ```compile_fail
    /// # use kithara_command::{Due, Protocol};
    /// fn move_basis<P: Protocol>(due: &mut Due<'_, P>) {
    ///     due.basis_mut()[0].1 = None;
    /// }
    /// ```
    pub fn apply(mut self, data: P::Applied) {
        self.inbox.docket.ledger.record(&self.basis, self.seq);
        self.outcome = Some(Outcome::Applied { data, at: self.at });
    }

    /// Answers the whole batch with the executor's refusal.
    pub fn refuse(mut self, refusal: P::Refusal) {
        self.outcome = Some(Outcome::Rejected(Rejection::Refused(refusal)));
    }

    /// Parks the judged batch, retaining its credit until it is resumed or retired.
    pub fn defer(mut self) -> Seq {
        self.outcome = None;
        self.inbox.docket.park(Sent {
            seq: self.seq,
            when: When::Deferred,
            batch: Batch {
                basis: mem::take(&mut self.basis),
                commands: mem::take(&mut self.commands),
            },
        });
        self.seq
    }
}

impl<P: Protocol> Drop for Due<'_, P> {
    fn drop(&mut self) {
        let Some(outcome) = self.outcome.take() else {
            return;
        };
        let applied = matches!(outcome, Outcome::Applied { .. });
        self.inbox.reply(Receipt {
            seq: self.seq,
            outcome,
            batch: Batch {
                basis: mem::take(&mut self.basis),
                commands: mem::take(&mut self.commands),
            },
        });
        if applied {
            self.inbox.reject_outdated();
        }
    }
}

impl<P: Protocol> Deferred<'_, P> {
    /// The batch's send number.
    #[must_use]
    pub fn seq(&self) -> Seq {
        self.seq
    }

    /// The batch's declared basis, not yet judged.
    #[must_use]
    pub fn basis(&self) -> &[(P::Target, Option<Seq>)] {
        &self.basis
    }

    /// Commands the executor may inspect before parking.
    #[must_use]
    pub fn commands(&self) -> &[P::Command] {
        &self.commands
    }

    /// Parks unless the basis is already outdated, in which case it answers Stale.
    pub fn park(mut self) -> Option<Seq> {
        if self.inbox.docket.ledger.outdates(&self.basis) {
            self.outcome = Some(Outcome::Rejected(Rejection::Stale));
            return None;
        }
        self.outcome = None;
        self.inbox.docket.park(Sent {
            seq: self.seq,
            when: When::Deferred,
            batch: Batch {
                basis: mem::take(&mut self.basis),
                commands: mem::take(&mut self.commands),
            },
        });
        Some(self.seq)
    }

    /// Refuses without parking or shifting any target.
    pub fn refuse(mut self, refusal: P::Refusal) {
        self.outcome = Some(Outcome::Rejected(Rejection::Refused(refusal)));
    }
}

impl<P: Protocol> Drop for Deferred<'_, P> {
    fn drop(&mut self) {
        let Some(outcome) = self.outcome.take() else {
            return;
        };
        self.inbox.reply(Receipt {
            seq: self.seq,
            outcome,
            batch: Batch {
                basis: mem::take(&mut self.basis),
                commands: mem::take(&mut self.commands),
            },
        });
    }
}
