//! What a player sends through and what comes back to it.

use kithara_command::{Batch, Receipt, Rejection, SendError, Sender, Seq, When};
use kithara_render::{
    DispatcherProtocol,
    bridge::{DeckEvent, DeckPart, DeckProtocol, Slot},
};
use kithara_signal::SessionFrame;

use crate::{PlayError, ResourceLoad};

/// One loaded track or a deck built of them: it changes its own state on the
/// owner's thread and reaches the executors only through the [`Outbox`] it is
/// lent.
pub trait Player<S> {
    /// What the player is told to do.
    type Command;
    /// What the player shows of itself.
    type Snapshot;

    /// The frame the player can enter on, nearest to `bound` from the side it
    /// names.
    fn entry(&self, bound: Bound) -> SessionFrame;

    /// Applies `command` and returns the number of the batch it became, if one
    /// went out on its own.
    ///
    /// # Errors
    ///
    /// Returns why nothing was sent: a refused change, or a queue with no
    /// room.
    fn apply(
        &mut self,
        command: Self::Command,
        out: &mut Outbox<'_, S>,
    ) -> Result<Option<Seq>, PlayError>;

    /// Takes what an executor answered: what the player holds from it moves in,
    /// and the outcome goes up.
    fn settle(&mut self, receipt: TrackReceipt<'_, S>, out: &mut Outbox<'_, S>) -> Settled;

    /// One step of session time: deadlines and the receipts of the player's
    /// own lane.
    fn tick(&mut self, now: SessionFrame, out: &mut Outbox<'_, S>);

    fn snapshot(&self) -> Self::Snapshot;
}

/// Which side of a frame an entry is looked for on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Bound {
    /// The earliest entry no earlier than the frame: a press, a resume.
    AtOrAfter(SessionFrame),
    /// The latest entry no later than the frame: an automatic transition.
    AtOrBefore(SessionFrame),
}

/// The queues a player sends to: the mixer of its deck and the dispatcher that
/// opens sources. The owner lends it for one pass.
///
/// Inside [`Outbox::together`] every deck part goes into one batch, so the
/// halves of a transition two players send apply on one frame or not at all.
pub struct Outbox<'a, S> {
    deck: &'a mut Sender<DeckProtocol>,
    dispatcher: &'a mut Sender<DispatcherProtocol<ResourceLoad<S>>>,
    group: Option<Group>,
}

/// The batch [`Outbox::together`] is collecting.
struct Group {
    at: When<SessionFrame>,
    basis: Vec<(Slot, Option<Seq>)>,
    parts: Vec<DeckPart>,
}

impl<'a, S> Outbox<'a, S> {
    #[must_use]
    pub fn new(
        deck: &'a mut Sender<DeckProtocol>,
        dispatcher: &'a mut Sender<DispatcherProtocol<ResourceLoad<S>>>,
    ) -> Self {
        Self {
            deck,
            dispatcher,
            group: None,
        }
    }

    /// Runs `send` with every deck part it sends collected into one batch
    /// that applies at `at`, and sends that batch once `send` returns. A
    /// refusal from `send` sends nothing.
    ///
    /// # Errors
    ///
    /// Returns the refusal of `send`, or the deck's when it has no room for the
    /// batch; nothing was sent then.
    pub fn together<R>(
        &mut self,
        at: When<SessionFrame>,
        send: impl FnOnce(&mut Self) -> Result<R, PlayError>,
    ) -> Result<(R, Option<Seq>), PlayError> {
        self.group = Some(Group {
            at,
            basis: Vec::new(),
            parts: Vec::new(),
        });
        let sent = send(self);
        let group = self.group.take();
        let value = sent?;
        let Some(Group { at, basis, parts }) = group.filter(|group| !group.parts.is_empty()) else {
            return Ok((value, None));
        };
        let seq = deck_sent(self.deck.send(at, Batch { basis, commands: parts }))?;
        Ok((value, Some(seq)))
    }

    /// Sends `parts` to the deck to apply at `at`, each slot they name on the
    /// basis of the last batch sent for it; inside [`Outbox::together`] they
    /// join its batch and no number comes back.
    ///
    /// # Errors
    ///
    /// Returns [`PlayError::Full`] when the deck has no room, and
    /// [`PlayError::Internal`] for a part of a group at another moment.
    pub(crate) fn deck(
        &mut self,
        at: When<SessionFrame>,
        parts: Vec<DeckPart>,
    ) -> Result<Option<Seq>, PlayError> {
        let deck = &*self.deck;
        if let Some(group) = &mut self.group {
            if group.at != at {
                return Err(PlayError::Internal(format!(
                    "a part at {at:?} joined a batch at {:?}",
                    group.at
                )));
            }
            for slot in parts.iter().flat_map(slots) {
                if !group.basis.iter().any(|&(named, _)| named == slot) {
                    group.basis.push((slot, deck.basis(slot)));
                }
            }
            group.parts.extend(parts);
            return Ok(None);
        }
        let mut basis: Vec<(Slot, Option<Seq>)> = Vec::new();
        for slot in parts.iter().flat_map(slots) {
            if !basis.iter().any(|&(named, _)| named == slot) {
                basis.push((slot, deck.basis(slot)));
            }
        }
        deck_sent(self.deck.send(at, Batch { basis, commands: parts })).map(Some)
    }

    /// Asks the dispatcher to open `item`.
    ///
    /// # Errors
    ///
    /// Returns [`PlayError::Full`] when the dispatcher has no room.
    pub(crate) fn load(&mut self, item: ResourceLoad<S>) -> Result<Seq, PlayError> {
        let batch = Batch {
            basis: Vec::new(),
            commands: vec![item],
        };
        self.dispatcher
            .send(When::Next, batch)
            .map_err(|error| match error {
                SendError::Full(_) => PlayError::Full("dispatcher"),
                SendError::Target(_) | SendError::Closed(_) => PlayError::Closed,
            })
    }
}

/// The slots a part shifts the time of.
fn slots(part: &DeckPart) -> impl Iterator<Item = Slot> {
    let (first, second) = match *part {
        DeckPart::Attach { slot, .. }
        | DeckPart::Detach { slot }
        | DeckPart::Start { slot, .. }
        | DeckPart::Stop { slot, .. }
        | DeckPart::Fade { slot, .. }
        | DeckPart::Seek { slot, .. }
        | DeckPart::Rate { slot, .. }
        | DeckPart::Replace { slot, .. } => (Some(slot), None),
        DeckPart::Chain { from, to } => (Some(from), Some(to)),
        DeckPart::Mix(_) | DeckPart::Eq(_) | DeckPart::Released(_) => (None, None),
    };
    first.into_iter().chain(second)
}

fn deck_sent(sent: Result<Seq, SendError<DeckProtocol>>) -> Result<Seq, PlayError> {
    sent.map_err(|error| match error {
        SendError::Full(_) => PlayError::Full("deck"),
        SendError::Target(batch) => PlayError::Internal(format!(
            "a deck batch names a slot outside the mixer: {:?}",
            batch.basis
        )),
        SendError::Closed(_) => PlayError::Closed,
    })
}

/// What became of a batch, as a player reports it up.
#[derive(Clone, Debug)]
pub enum Settled {
    /// The receipt is not about this player, or its outcome has not come yet.
    Pending,
    Applied { seq: Seq, at: SessionFrame },
    Rejected { seq: Seq, reason: Rejection<PlayError> },
}

/// What comes back to a player: a receipt of its deck's mixer, shared by every
/// player whose slot the batch names; a receipt of the dispatcher; or an event
/// of its slot. Its own lane's receipts it reads itself.
pub enum TrackReceipt<'r, S> {
    Deck(&'r Receipt<DeckProtocol>),
    Loaded(Receipt<DispatcherProtocol<ResourceLoad<S>>>),
    Event(DeckEvent),
}

impl<S> TrackReceipt<'_, S> {
    /// Whether this is about `slot`: a deck batch whose basis names it, or an
    /// event of it.
    #[must_use]
    pub fn names(&self, slot: Slot) -> bool {
        match self {
            Self::Deck(receipt) => receipt.batch().basis.iter().any(|&(named, _)| named == slot),
            Self::Event(
                DeckEvent::Ended { slot: named, .. }
                | DeckEvent::Failed { slot: named, .. }
                | DeckEvent::Faded { slot: named, .. }
                | DeckEvent::Underrun { slot: named, .. },
            ) => *named == slot,
            Self::Loaded(_) => false,
        }
    }
}

/// `rejection` as it goes up, the executor's own reason made a [`PlayError`].
pub(crate) fn rejection<R>(
    rejection: &Rejection<R>,
    refusal: impl FnOnce(&R) -> PlayError,
) -> Rejection<PlayError> {
    match rejection {
        Rejection::Late => Rejection::Late,
        Rejection::Stale => Rejection::Stale,
        Rejection::Unanswered => Rejection::Unanswered,
        Rejection::Refused(reason) => Rejection::Refused(refusal(reason)),
    }
}
