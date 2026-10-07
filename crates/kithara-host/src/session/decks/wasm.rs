use kithara_warp::BeatGridId;

use super::{Deck, Decks};

impl IntoIterator for Decks {
    type Item = (BeatGridId, Deck);
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}
