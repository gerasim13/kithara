use kithara_play::PlayError;
use kithara_warp::BeatGridId;

use super::{Deck, Decks};
use crate::session::SessionError;

impl Decks {
    /// Closes the deck `id` where it is held, so a failed close leaves it
    /// held.
    pub(crate) fn close(&mut self, id: BeatGridId) -> Result<(), PlayError> {
        self.0
            .iter_mut()
            .find(|(held, _)| *held == id)
            .ok_or(SessionError::DeckNotFound(id))?
            .1
            .close()
    }
}

impl IntoIterator for Decks {
    type Item = (BeatGridId, Deck);
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}
