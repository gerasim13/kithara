use kithara_play::{PlayError, player::Player};
use kithara_warp::BeatGridId;
use tracing::warn;

use crate::session::SessionError;

/// A deck as a Host holds it: the player or decorator it was handed.
pub(crate) type Deck = Box<dyn Player>;

/// The decks a Host holds, each ticked once per pass.
#[derive(Default)]
pub(crate) struct Decks(pub(super) Vec<(BeatGridId, Deck)>);

impl Decks {
    /// Holds `deck` and runs the commands posted to it before it was held,
    /// which woke no one.
    pub(crate) fn hold(&mut self, id: BeatGridId, mut deck: Deck) {
        deck.drain();
        self.0.push((id, deck));
    }

    /// Runs the commands posted to the deck `id`. A deck let go before its
    /// wake arrived dropped those commands as it was released.
    pub(crate) fn drain(&mut self, id: BeatGridId) {
        if let Some((_, deck)) = self.0.iter_mut().find(|(held, _)| *held == id) {
            deck.drain();
        }
    }

    /// Lets go of the deck `id` and hands it back released.
    pub(crate) fn release(&mut self, id: BeatGridId) -> Result<Deck, PlayError> {
        let index = self
            .0
            .iter()
            .position(|(held, _)| *held == id)
            .ok_or(SessionError::DeckNotFound(id))?;
        let (_, mut deck) = self.0.remove(index);
        deck.release();
        Ok(deck)
    }

    /// Releases every deck while keeping it here, so none takes a command
    /// after its holder stopped.
    pub(crate) fn release_all(&mut self) {
        for (_, deck) in &mut self.0 {
            deck.release();
        }
    }

    /// Ticks every deck once; a deck whose tick fails stays held.
    pub(crate) fn tick(&mut self) {
        for (id, deck) in &mut self.0 {
            if let Err(error) = deck.tick() {
                warn!(?id, %error, "host deck tick failed");
            }
        }
    }
}
