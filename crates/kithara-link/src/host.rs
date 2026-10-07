use std::{marker::PhantomData, num::NonZeroU32};

use kithara_command::{Rejection, Seq, When};
use kithara_host::{
    DeckId, HostCommand, HostOwner, HostSettingsChange, HostSettingsExec, HostSettled, api::Tempo,
};
use kithara_play::{DeckPass, Outbox, PlayError};
use kithara_signal::{FrameCount, SessionFrame};

use crate::{GridAnswer, LinkedDeck, TempoTrajectory};

/// A tempo batch whose projected step is retained until the Host answers it.
#[derive(Clone, Copy, Debug)]
pub struct PendingTempo {
    pub seq: Seq,
    pub frame: SessionFrame,
    pub value: Tempo,
    pub at: When<SessionFrame>,
}

/// Owner commands for a linked Host and the decks it holds.
pub enum LinkedHostCommand<S> {
    /// A Host command: registration and tempo go through the decorator,
    /// the rest to the owner it wraps.
    Host(HostCommand<S, dyn LinkedDeck<S>>),
    Sync {
        deck: DeckId,
        on: bool,
    },
    Grid {
        deck: DeckId,
        answer: GridAnswer,
    },
}

impl<S> From<HostCommand<S, dyn LinkedDeck<S>>> for LinkedHostCommand<S> {
    fn from(command: HostCommand<S, dyn LinkedDeck<S>>) -> Self {
        Self::Host(command)
    }
}

/// Decorates the session owner with projected tempo and synchronized deck retiming.
pub struct LinkedHost<S, H> {
    inner: H,
    trajectory: TempoTrajectory,
    tempo: Vec<PendingTempo>,
    schema: PhantomData<fn() -> S>,
}

impl<S, H> LinkedHost<S, H> {
    /// Starts with the Host's configured tempo anchor and no tempo batches in flight.
    #[must_use]
    pub fn new(inner: H, trajectory: TempoTrajectory) -> Self {
        Self {
            inner,
            trajectory,
            tempo: Vec::new(),
            schema: PhantomData,
        }
    }
}

impl<S: 'static, H: HostOwner<S, Deck = dyn LinkedDeck<S>>> HostOwner<S> for LinkedHost<S, H> {
    type Command = LinkedHostCommand<S>;
    type Deck = dyn LinkedDeck<S>;

    fn apply(&mut self, command: Self::Command) -> Result<Option<Seq>, PlayError> {
        match command {
            LinkedHostCommand::Host(HostCommand::Register { id, deck }) => {
                self.register(id, deck).map(|()| None)
            }
            LinkedHostCommand::Host(HostCommand::Configure(change, at)) => {
                self.exec(change, at, &mut ())
            }
            LinkedHostCommand::Host(command) => self.inner.apply(H::Command::from(command)),
            LinkedHostCommand::Sync { deck, on } => {
                let mut sent = Ok(None);
                self.inner.with_deck(deck, &mut |deck, out, _pass| {
                    sent = deck.sync(on, out);
                })?;
                sent
            }
            LinkedHostCommand::Grid { deck, answer } => {
                self.inner.with_deck(deck, &mut |deck, out, _pass| {
                    deck.grid(answer.clone(), out);
                })?;
                Ok(None)
            }
        }
    }

    fn register(&mut self, id: DeckId, deck: Box<Self::Deck>) -> Result<(), PlayError> {
        self.inner.register(id, deck)?;
        let trajectory = &self.trajectory;
        self.inner.with_deck(id, &mut |deck, out, pass| {
            deck.retime(trajectory, pass.now, out);
        })
    }

    fn each_deck(
        &mut self,
        visit: &mut dyn FnMut(DeckId, &mut Self::Deck, &mut Outbox<'_, S>, DeckPass<'_>),
    ) {
        self.inner.each_deck(visit);
    }

    fn with_deck(
        &mut self,
        id: DeckId,
        visit: &mut dyn FnMut(&mut Self::Deck, &mut Outbox<'_, S>, DeckPass<'_>),
    ) -> Result<(), PlayError> {
        self.inner.with_deck(id, visit)
    }

    fn clock(&self) -> Option<(SessionFrame, FrameCount)> {
        self.inner.clock()
    }

    fn host_room(&self) -> usize {
        self.inner.host_room()
    }

    fn pass(&mut self) -> Vec<HostSettled> {
        let settled = self.inner.pass();
        for answer in &settled {
            let HostSettled::Settings {
                seq,
                change,
                outcome,
            } = answer;
            if !matches!(change, HostSettingsChange::Tempo(_)) {
                continue;
            }
            let Some(index) = self.tempo.iter().position(|pending| pending.seq == *seq) else {
                continue;
            };
            let pending = self.tempo.remove(index);
            if matches!(outcome, Err(Rejection::Late | Rejection::Refused(_))) {
                self.trajectory.withdraw(pending.frame);
                let newer_next = self
                    .tempo
                    .iter()
                    .any(|tempo| tempo.seq > pending.seq && tempo.at == When::Next);
                if pending.at == When::Next && !newer_next {
                    todo!(
                        "Repeat this Next tempo from frame/room admission on a fresh F, unless a newer Next supersedes it (spec §4.6 step 4)"
                    )
                }
                todo!(
                    "At refusal, or superseded Next: retime synchronized decks to the withdrawn trajectory at a fresh F and correct accumulated phase without a jump (spec §4.6 step 4)"
                )
            }
        }
        settled
    }
}

impl<S: 'static, H: HostOwner<S, Deck = dyn LinkedDeck<S>>> HostSettingsExec<()>
    for LinkedHost<S, H>
{
    type At = When<SessionFrame>;
    type Output = Result<Option<Seq>, PlayError>;

    fn exec_sample_rate(&mut self, value: NonZeroU32, at: Self::At, cx: &mut ()) -> Self::Output {
        self.inner.exec_sample_rate(value, at, cx)
    }

    fn exec_tempo(&mut self, value: Tempo, at: Self::At, cx: &mut ()) -> Self::Output {
        let Some((now, delivery)) = self.inner.clock() else {
            todo!(
                "Apply an untimed initial tempo to the trajectory and Host settings without a render graph; At is Untimed (spec §4.6/§4.8)"
            )
        };
        let mut lead = None;
        self.inner.each_deck(&mut |_id, deck, _out, _pass| {
            if deck.synced() {
                lead = lead.max(deck.lead());
            }
        });
        let bound = now + lead.unwrap_or(delivery);
        let frame = match at {
            When::Next => bound,
            When::At(frame) if frame < bound => return Err(PlayError::Late),
            When::At(frame) => frame,
        };
        if self.inner.host_room() == 0 {
            return Err(PlayError::Full("host"));
        }
        let mut lanes_full = false;
        self.inner.each_deck(&mut |_id, deck, _out, _pass| {
            if deck.synced() && deck.lane_room() == 0 {
                lanes_full = true;
            }
        });
        if lanes_full {
            return Err(PlayError::Full("lane"));
        }
        self.trajectory
            .push(frame, value)
            .map_err(|error| PlayError::Internal(error.to_string()))?;
        let trajectory = &self.trajectory;
        self.inner.each_deck(&mut |_id, deck, out, _pass| {
            if deck.synced() {
                deck.retime(trajectory, frame, out);
            }
        });
        match self.inner.exec_tempo(value, When::At(frame), cx) {
            Ok(Some(seq)) => {
                self.tempo.push(PendingTempo {
                    seq,
                    frame,
                    value,
                    at,
                });
                Ok(Some(seq))
            }
            Ok(None) => {
                todo!("Settle a tempo that the inner owner applied without a batch (spec §4.6)")
            }
            Err(error) => {
                self.trajectory.withdraw(frame);
                let _ = error;
                todo!(
                    "Compensate already-sent deck retimes before returning the inner send refusal (spec §4.6 step 4)"
                )
            }
        }
    }

    fn exec_live(&mut self, change: HostSettingsChange, at: Self::At, cx: &mut ()) -> Self::Output {
        self.inner.exec_live(change, at, cx)
    }
}
