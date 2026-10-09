use std::{num::NonZeroU32, task::Waker};

use kithara_bufpool::HasPool;
use kithara_command::{Outcome, Rejection, Seq};
use kithara_platform::maybe_send::MaybeSend;
use kithara_play::{
    Bound, DeckControl, DeckEvent, DeckMixerConfig, DeckPass, HostedDeck, LoadRefusal, Outbox,
    OutputSnapshot, PlayError, PlayWorker, Player, Settled, TrackCommand, TrackFactory, TrackReceipt,
    TrackStatus as PlayingStatus,
};
use kithara_signal::SessionFrame;
use tracing::warn;

use super::{
    Queue, QueueCommand, QueueControl, QueueSnapshot, command::play_error,
    slots::{LoadState, Role},
};
use crate::{ActionAtItemEnd, QueueError, QueueEvent, TrackStatus, loader};

impl<S, F> DeckControl for Queue<S, F>
where
    S: HasPool<u8> + Send + Sync + 'static,
    F: TrackFactory<S>,
{
    type Control = QueueControl<S>;

    fn control(&self) -> Self::Control {
        Queue::control(self)
    }
}

impl<S, F> Player<S> for Queue<S, F>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
    F: TrackFactory<S>,
{
    type Command = QueueCommand<S>;
    type Snapshot = QueueSnapshot<S>;

    fn entry(&self, bound: Bound) -> Option<SessionFrame> {
        self.current_track().and_then(|track| track.entry(bound))
    }

    fn apply(
        &mut self,
        command: Self::Command,
        out: &mut Outbox<'_, S>,
    ) -> Result<Option<Seq>, PlayError> {
        let output = out.pass().map(|pass| *pass.output);
        self.apply_with_output(command, output.as_ref(), out)
    }

    fn settle(&mut self, receipt: TrackReceipt<'_, S>, out: &mut Outbox<'_, S>) -> Settled {
        let output = out.pass().map(|pass| *pass.output);
        self.settle_with_output(receipt, output.as_ref(), out)
    }

    fn tick(&mut self, now: SessionFrame, out: &mut Outbox<'_, S>) {
        let output = out.pass().map(|pass| *pass.output);
        self.tick_with_output(now, output.as_ref(), out);
    }

    fn snapshot(&self) -> Self::Snapshot {
        self.queue_snapshot()
    }
}

impl<S, F> Queue<S, F>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
    F: TrackFactory<S>,
{
    fn apply_with_output(
        &mut self,
        command: QueueCommand<S>,
        output: Option<&OutputSnapshot>,
        out: &mut Outbox<'_, S>,
    ) -> Result<Option<Seq>, PlayError> {
        let result = self.apply_command(command, output, out).map_err(play_error);
        self.publish();
        result
    }

    fn settle_with_output(
        &mut self,
        receipt: TrackReceipt<'_, S>,
        output: Option<&OutputSnapshot>,
        out: &mut Outbox<'_, S>,
    ) -> Settled {
        let mut outcomes = Vec::new();
        match receipt {
            TrackReceipt::Deck {
                seq,
                outcome,
                batch,
            } => {
                for index in self
                    .active
                    .indices(|active| batch.basis.iter().any(|&(slot, _)| slot == active.slot))
                {
                    if let Some(active) = self.active.get_mut(index) {
                        outcomes.push((
                            index,
                            active.track.settle(
                                TrackReceipt::Deck {
                                    seq,
                                    outcome,
                                    batch: &mut *batch,
                                },
                                out,
                            ),
                        ));
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
                self.item_event(event, output, out);
            }
            TrackReceipt::Loaded(receipt) => {
                let seq = receipt.seq();
                if let Some(index) = self
                    .active
                    .position(|active| active.load == Some(LoadState::Opening(seq)))
                {
                    let opened = matches!(
                        receipt.outcome(),
                        Outcome::Applied { .. }
                    );
                    let retry = self.classify_load(index, receipt.outcome());
                    if let Some(active) = self.active.get_mut(index) {
                        let settled = active.track.settle(TrackReceipt::Loaded(receipt), out);
                        if retry {
                            if let Err(error) = self.retry_load(index, output, out) {
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
                            if opened {
                                self.accept_load(index, seq);
                            }
                            outcomes.push((index, settled));
                        }
                    }
                }
            }
        }
        let mut result = Settled::Pending;
        for (index, settled) in outcomes {
            let loading = self
                .active
                .get(index)
                .and_then(|active| active.load)
                .map(LoadState::seq);
            let rescheduling = self.target.and_then(|target| target.stale);
            if let Err(error) = self.transition_settled(index, &settled, out) {
                let seq = match settled {
                    Settled::Applied { seq, .. } | Settled::Rejected { seq, .. } => Some(seq),
                    Settled::Pending => None,
                };
                if let Some(seq) = seq {
                    result = Settled::Rejected {
                        seq,
                        reason: Rejection::Refused(error),
                    };
                }
                continue;
            }
            match &settled {
                Settled::Applied { seq, .. } => {
                    if !matches!(result, Settled::Rejected { .. }) && loading != Some(*seq) {
                        result = settled;
                    }
                }
                Settled::Rejected { seq, reason } => {
                    let retrying = self.target.is_some_and(|target| target.retry == Some(*seq));
                    if !retrying
                        && (rescheduling != Some(*seq) || !matches!(reason, Rejection::Stale))
                    {
                        result = settled;
                    }
                }
                Settled::Pending => {}
            }
        }
        if let Err(error) = self.release_tails(out) {
            warn!(%error, "outgoing queue track could not release");
        }
        if let Err(error) = self.transition_loaded(out) {
            warn!(%error, "loaded queue target could not enter");
        }
        self.reap_released();
        self.publish();
        result
    }

    fn tick_with_output(
        &mut self,
        now: SessionFrame,
        output: Option<&OutputSnapshot>,
        out: &mut Outbox<'_, S>,
    ) {
        if let Some((_, delivery)) = self.clock {
            self.clock = Some((now, delivery));
        }
        for active in self.active.iter_mut() {
            active.track.tick(now, out);
        }
        if let Err(error) = self.tick_deadlines(now, output, out) {
            warn!(%error, "queue deadline could not advance");
        }
        if let Err(error) = self.transition_loaded(out) {
            warn!(%error, "loaded queue target could not enter");
        }
        self.publish();
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

    fn worker(&self) -> Option<&PlayWorker<S>> {
        self.config.prep.as_ref().map(|prep| &prep.worker)
    }

    fn resource_prep(&self) -> Option<&kithara_play::ResourcePrep<S>> {
        self.config.prep.as_ref()
    }

    fn drain(&mut self, pass: DeckPass<'_>, out: &mut Outbox<'_, S>) {
        self.accept_host_pass(pass, out);
        for post in self.mailbox.drain() {
            if let Err(error) = self.validate_command(&post.command) {
                self.publish();
                post.answer.answer(Err(error));
                continue;
            }
            match self.apply_with_output(post.command, Some(pass.output), out) {
                Ok(_) => post.answer.answer(Ok(())),
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
        self.accept_host_pass(pass, out);
        self.settle_with_output(receipt, Some(pass.output), out);
    }

    fn tick(&mut self, pass: DeckPass<'_>, out: &mut Outbox<'_, S>) {
        self.accept_host_pass(pass, out);
        self.tick_with_output(pass.now, Some(pass.output), out);
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
    fn accept_host_pass(&mut self, pass: DeckPass<'_>, out: &mut Outbox<'_, S>) {
        self.clock = out.pass().map(|pass| (pass.now, pass.delivery));
        if self.clock.is_none() {
            return;
        }
        if self.deck.mixer.sample_rate != 0
            && self.deck.mixer.sample_rate != pass.deck.sample_rate
            && let Some(rate) = NonZeroU32::new(pass.deck.sample_rate)
            && let Err(error) = self.set_host_rate(rate, out)
        {
            warn!(%error, "queue tracks could not adopt the output rate");
            return;
        }
        self.deck.mix = pass.mix;
        self.deck.suspended = pass.suspended;
        self.deck.mixer.clone_from(pass.deck);
    }

    fn set_host_rate(
        &mut self,
        rate: NonZeroU32,
        out: &mut Outbox<'_, S>,
    ) -> Result<(), PlayError> {
        let loaded = self.active.indices(|active| {
            matches!(
                active.track.snapshot().as_ref().status,
                PlayingStatus::Loaded
                    | PlayingStatus::Playing { .. }
                    | PlayingStatus::Paused { .. }
                    | PlayingStatus::Faded { .. }
                    | PlayingStatus::Ended { .. }
            )
        });
        if out.deck_available() < loaded.len() {
            return Err(PlayError::Full("deck"));
        }
        for index in &loaded {
            let active = self.active.get(*index).ok_or(PlayError::NoActiveSlot)?;
            if active.track.snapshot().as_ref().lane_room == 0 {
                return Err(PlayError::Full("lane"));
            }
        }
        for index in loaded {
            self.active
                .get_mut(index)
                .ok_or(PlayError::NoActiveSlot)?
                .track
                .apply(TrackCommand::SetHostRate { rate }, out)?;
        }
        Ok(())
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

    fn item_event(&mut self, event: DeckEvent, output: Option<&OutputSnapshot>, out: &mut Outbox<'_, S>) {
        if let DeckEvent::Failed { slot, at, fault } = event {
            let Some(active) = self
                .active
                .iter()
                .find(|active| active.slot == slot && active.role == Role::Current)
            else {
                return;
            };
            let snapshot = active.track.snapshot();
            if !matches!(snapshot.as_ref().status, kithara_play::TrackStatus::Failed { at: failed_at, fault: actual } if failed_at == at && actual == fault)
            {
                return;
            }
            let id = active.item;
            if self
                .track(id)
                .is_some_and(|entry| matches!(entry.status, TrackStatus::Failed(_)))
            {
                return;
            }
            let reason = fault.to_string();
            self.tracks
                .set_status(id, TrackStatus::Failed(reason.clone()));
            self.announce(QueueEvent::TrackLoadFailed {
                id,
                reason,
                auto_skipped: self.config.action_at_item_end == ActionAtItemEnd::Advance,
            });
            if self.target.is_none() && self.config.action_at_item_end == ActionAtItemEnd::Advance {
                match self.next_target(
                    super::Transition::None,
                    crate::AdvanceReason::TrackFailed,
                    true,
                    output,
                    out,
                ) {
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
mod terminal_tests;

#[cfg(all(test, not(target_arch = "wasm32")))]
mod entry_tests;
