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
    pub(super) fn hold(&mut self, id: BeatGridId, deck: Deck) {
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

    pub(super) fn release(&mut self, id: BeatGridId) -> Result<Deck, PlayError> {
        let index = self
            .0
            .iter()
            .position(|(held, _)| *held == id)
            .ok_or(SessionError::DeckNotFound(id))?;
        Ok(self.0.remove(index).1)
    }

    /// Ticks every deck once; a deck whose tick fails stays held.
    pub(super) fn tick(&self) {
        for (id, deck) in &self.0 {
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
