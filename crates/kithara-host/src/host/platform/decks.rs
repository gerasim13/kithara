use kithara_play::{PlayError, player::Player};
use kithara_warp::BeatGridId;
use tracing::warn;

use crate::session::SessionError;

/// A deck as a Host holds it: the player or decorator it was handed.
pub(super) type Deck = Box<dyn Player>;

/// The decks a Host holds, each ticked once per pass.
#[derive(Default)]
pub(super) struct Decks(Vec<(BeatGridId, Deck)>);

impl Decks {
    /// Holds `deck` and runs the commands posted to it before it was held,
    /// which woke no one.
    pub(super) fn hold(&mut self, id: BeatGridId, mut deck: Deck) {
        deck.drain();
        self.0.push((id, deck));
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Closes the deck `id` where it is held, so a failed close leaves it
    /// held.
    #[cfg(target_arch = "wasm32")]
    pub(super) fn close(&mut self, id: BeatGridId) -> Result<(), PlayError> {
        self.0
            .iter_mut()
            .find(|(held, _)| *held == id)
            .ok_or(SessionError::DeckNotFound(id))?
            .1
            .close()
    }

    /// Runs the commands posted to the deck `id`. A deck let go before its
    /// wake arrived dropped those commands as it was released.
    pub(super) fn drain(&mut self, id: BeatGridId) {
        if let Some((_, deck)) = self.0.iter_mut().find(|(held, _)| *held == id) {
            deck.drain();
        }
    }

    /// Lets go of the deck `id` and hands it back released.
    pub(super) fn release(&mut self, id: BeatGridId) -> Result<Deck, PlayError> {
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
    pub(super) fn release_all(&mut self) {
        for (_, deck) in &mut self.0 {
            deck.release();
        }
    }

    /// Ticks every deck once; a deck whose tick fails stays held.
    pub(super) fn tick(&mut self) {
        for (id, deck) in &mut self.0 {
            if let Err(error) = deck.tick() {
                warn!(?id, %error, "host deck tick failed");
            }
        }
    }
}

impl IntoIterator for Decks {
    type Item = (BeatGridId, Deck);
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}
