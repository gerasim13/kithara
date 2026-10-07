mod core;
#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(target_arch = "wasm32")]
mod wasm;

pub(crate) use core::{Deck, Decks};

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use native::{DeckInbox, DeckMsg, SessionDecks};
