use std::task::Waker;

use kithara_bufpool::HasPool;
use kithara_command::{Outcome, Rejection, Seq};
use kithara_platform::maybe_send::MaybeSend;
use kithara_play::{
    Bound, DeckEvent, DeckMixerConfig, DeckPass, HostedDeck, LoadRefusal, Outbox, PlayError,
    Player, Settled, TrackFactory, TrackReceipt,
};
use kithara_signal::SessionFrame;
use tracing::warn;

use super::{Queue, QueueCommand, QueueSnapshot, command::play_error, slots::Role};
use crate::{ActionAtItemEnd, QueueError, QueueEvent, TrackStatus, loader};

impl<S, F> Player<S> for Queue<S, F>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
    F: TrackFactory<S>,
{
    type Command = QueueCommand<S>;
    type Snapshot = QueueSnapshot<S>;

    fn entry(&self, bound: Bound) -> SessionFrame {
        self.current_track().map_or_else(
            || match bound {
                Bound::AtOrAfter(frame) | Bound::AtOrBefore(frame) => frame,
            },
            |track| track.entry(bound),
        )
    }

    fn apply(
        &mut self,
        command: Self::Command,
        out: &mut Outbox<'_, S>,
    ) -> Result<Option<Seq>, PlayError> {
        let result = self.apply_command(command, out).map_err(play_error);
        self.publish();
        result
    }

    fn settle(&mut self, receipt: TrackReceipt<'_, S>, out: &mut Outbox<'_, S>) -> Settled {
        let mut outcomes = Vec::new();
        match receipt {
            TrackReceipt::Deck(receipt) => {
                let named: TrackReceipt<'_, S> = TrackReceipt::Deck(receipt);
                for index in self.active.indices(|active| named.names(active.slot)) {
                    if let Some(active) = self.active.get_mut(index) {
                        outcomes
                            .push((index, active.track.settle(TrackReceipt::Deck(receipt), out)));
                    }
                }
            }
            TrackReceipt::Event(event) => {
                let named: TrackReceipt<'_, S> = TrackReceipt::Event(event);
                for index in self.active.indices(|active| named.names(active.slot)) {
                    if let Some(active) = self.active.get_mut(index) {
                        outcomes
                            .push((index, active.track.settle(TrackReceipt::Event(event), out)));
                    }
                }
                self.item_event(event, out);
            }
            TrackReceipt::Loaded(receipt) => {
                let seq = receipt.seq();
                if let Some(index) = self.active.position(|active| active.load == Some(seq)) {
                    let retry = self.classify_load(index, receipt.outcome());
                    if let Some(active) = self.active.get_mut(index) {
                        let settled = active.track.settle(TrackReceipt::Loaded(receipt), out);
                        if retry {
                            if let Err(error) = self.retry_load(index, seq, out) {
                                let error = play_error(error);
                                outcomes.push((
                                    index,
                                    Settled::Rejected {
                                        seq,
                                        reason: Rejection::Refused(error),
                                    },
                                ));
                            }
                        } else {
                            outcomes.push((index, settled));
                        }
                    }
                }
            }
        }
        let mut result = Settled::Pending;
        for (index, settled) in outcomes {
            let loading = self.active.get(index).and_then(|active| active.load);
            let rescheduling = self.target.and_then(|target| target.stale);
            if let Err(error) = self.transition_settled(index, &settled, out) {
                let seq = match settled {
                    Settled::Applied { seq, .. } | Settled::Rejected { seq, .. } => Some(seq),
                    Settled::Pending => None,
                };
                if let Some(seq) = seq {
                    self.finish_answers(seq, Err(error.clone()));
                    result = Settled::Rejected {
                        seq,
                        reason: Rejection::Refused(error),
                    };
                }
                continue;
            }
            match &settled {
                Settled::Applied { seq, .. } => {
                    if loading != Some(*seq) {
                        self.finish_answers(*seq, Ok(()));
                    }
                    if !matches!(result, Settled::Rejected { .. }) && loading != Some(*seq) {
                        result = settled;
                    }
                }
                Settled::Rejected { seq, reason } => {
                    if rescheduling != Some(*seq) || !matches!(reason, Rejection::Stale) {
                        self.finish_answers(*seq, Err(refusal(reason)));
                        result = settled;
                    }
                }
                Settled::Pending => {}
            }
        }
        if let Err(error) = self.release_tails(out) {
            warn!(%error, "outgoing queue track could not release");
        }
        self.reap_released();
        self.publish();
        result
    }

    fn tick(&mut self, now: SessionFrame, out: &mut Outbox<'_, S>) {
        if let Some((_, delivery)) = self.clock {
            self.clock = Some((now, delivery));
        }
        for active in self.active.iter_mut() {
            active.track.tick(now, out);
        }
        if let Err(error) = self.tick_deadlines(now, out) {
            warn!(%error, "queue deadline could not advance");
        }
        if let Err(error) = self.transition_loaded(out) {
            warn!(%error, "loaded queue target could not enter");
        }
        self.publish();
    }

    fn snapshot(&self) -> Self::Snapshot {
        self.queue_snapshot()
    }
}

impl<S, F> HostedDeck<S> for Queue<S, F>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
    F: TrackFactory<S> + MaybeSend + 'static,
    F::Track: MaybeSend + 'static,
{
    fn mixer_config(&self) -> DeckMixerConfig {
        self.config.mixer
    }

    fn drain(&mut self, pass: DeckPass<'_>, out: &mut Outbox<'_, S>) {
        self.accept_pass(pass);
        for post in self.mailbox.drain() {
            if let Err(error) = self.validate_command(&post.command) {
                self.publish();
                post.answer.answer(Err(error));
                continue;
            }
            match Player::apply(self, post.command, out) {
                Ok(Some(seq)) => self.answers.push((seq, post.answer)),
                Ok(None) => post.answer.answer(Ok(())),
                Err(error) => post.answer.answer(Err(error.into())),
            }
        }
    }

    fn settle(
        &mut self,
        receipt: TrackReceipt<'_, S>,
        pass: DeckPass<'_>,
        out: &mut Outbox<'_, S>,
    ) {
        self.accept_pass(pass);
        Player::settle(self, receipt, out);
    }

    fn tick(&mut self, pass: DeckPass<'_>, out: &mut Outbox<'_, S>) {
        self.accept_pass(pass);
        Player::tick(self, pass.now, out);
    }

    fn close(&mut self, out: &mut Outbox<'_, S>) -> Result<(), PlayError> {
        self.close_tracks(out)?;
        self.publish();
        Ok(())
    }

    fn hold(&mut self, waker: Waker) {
        self.mailbox.hold(waker);
    }

    fn release(&mut self) {
        self.mailbox.release();
    }
}

impl<S, F> Queue<S, F>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
    F: TrackFactory<S>,
{
    pub(super) fn retarget_answers(&mut self, previous: Seq, next: Seq) {
        for (seq, _) in &mut self.answers {
            if *seq == previous {
                *seq = next;
            }
        }
    }

    pub(super) fn finish_answers(&mut self, seq: Seq, result: Result<(), PlayError>) {
        if !self.answers.iter().any(|(pending, _)| *pending == seq) {
            return;
        }
        self.publish();
        let mut index = 0;
        while index < self.answers.len() {
            if self.answers[index].0 == seq {
                let (_, answer) = self.answers.remove(index);
                answer.answer(result.clone().map_err(QueueError::from));
            } else {
                index += 1;
            }
        }
    }

    fn classify_load(
        &mut self,
        index: usize,
        outcome: &Outcome<kithara_play::DispatcherProtocol<kithara_play::ResourceLoad<S>>>,
    ) -> bool {
        let Some(active) = self.active.get(index) else {
            return false;
        };
        if active.role == Role::Leaving {
            return false;
        }
        let id = active.item;
        let rejected = match outcome {
            Outcome::Applied { .. } => return false,
            Outcome::Rejected(rejected) => rejected,
        };
        let Rejection::Refused(refusal) = rejected else {
            let error = match rejected {
                Rejection::Late => PlayError::Late,
                Rejection::Stale => PlayError::NotReady,
                Rejection::Unanswered => PlayError::Closed,
                Rejection::Refused(_) => return false,
            };
            self.tracks.fail(id, &error.into());
            return false;
        };
        if matches!(refusal, LoadRefusal::Cancelled) {
            self.tracks.set_status(id, TrackStatus::Cancelled);
            return false;
        }
        let wanted = self.target.is_some_and(|target| target.to == id)
            && matches!(active.role, Role::Incoming { .. });
        let error = QueueError::Resource(refusal.to_string());
        let retry = self
            .tracks
            .refused(id, &error, loader::asks_again(refusal), wanted);
        if !retry {
            self.announce(QueueEvent::TrackLoadFailed {
                id,
                reason: error.to_string(),
                auto_skipped: false,
            });
        }
        retry
    }

    fn item_event(&mut self, event: DeckEvent, out: &mut Outbox<'_, S>) {
        if let DeckEvent::Failed { slot, at, fault } = event {
            let Some(active) = self.active.iter().find(|active| active.slot == slot && active.role == Role::Current) else {
                return;
            };
            let snapshot = active.track.snapshot();
            if !matches!(snapshot.as_ref().status, kithara_play::TrackStatus::Failed { at: failed_at, fault: actual } if failed_at == at && actual == fault) {
                return;
            }
            let id = active.item;
            if self.track(id).is_some_and(|entry| matches!(entry.status, TrackStatus::Failed(_))) {
                return;
            }
            let reason = fault.to_string();
            self.tracks.set_status(id, TrackStatus::Failed(reason.clone()));
            self.announce(QueueEvent::TrackLoadFailed {
                id, reason, auto_skipped: self.config.action_at_item_end == ActionAtItemEnd::Advance,
            });
            if self.target.is_none() && self.config.action_at_item_end == ActionAtItemEnd::Advance {
                match self.next_target(super::Transition::None, crate::AdvanceReason::TrackFailed, true, out) {
                    Ok(Some(_)) => {}
                    Ok(None) => self.announce(QueueEvent::QueueEnded),
                    Err(error) => warn!(%error, "queue could not advance after source failure"),
                }
            }
            return;
        }
        if let DeckEvent::Ended { slot, .. } = event {
            let current = self
                .active
                .iter()
                .any(|active| active.slot == slot && active.role == Role::Current);
            if current
                && self.target.is_none()
                && self.config.action_at_item_end == ActionAtItemEnd::Advance
            {
                let ids = self.track_ids();
                let wrap = self.navigation.repeat_mode() == crate::RepeatMode::All;
                if self.navigation.next(&ids, true, wrap).is_none() {
                    self.announce(QueueEvent::QueueEnded);
                }
            }
        }
    }
}

pub(super) fn refusal(reason: &Rejection<PlayError>) -> PlayError {
    match reason {
        Rejection::Late => PlayError::Late,
        Rejection::Stale => PlayError::NotReady,
        Rejection::Unanswered => PlayError::Closed,
        Rejection::Refused(error) => error.clone(),
    }
}


#[cfg(test)]
#[path = "hosted_terminal_tests.rs"]
mod terminal_tests;
