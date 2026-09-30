use kithara::ui::render::{Published, WindowCommand};

use crate::{
    deck::{DeckId, EqMode},
    engine::MixCmd,
};

/// All GUI events flow through this enum.
///
/// Deck-scoped events carry the deck they address; blocks themselves emit
/// [`super::deck::DeckMsg`] and know nothing about deck identity.
#[derive(Debug, Clone)]
pub(crate) enum Message {
    BroadcastToggle,
    /// What the compiled UI published; settled against the document's
    /// writes, then translated by [`super::ui::translate`].
    Ui(Published),
    /// Event addressed to one deck.
    Deck(DeckId, super::deck::DeckMsg),
    /// Replace the EQ topology of every deck.
    SetEqMode(EqMode),
    /// Session-mix edit (crossfader, trim).
    Mix(MixCmd),
    /// Delete the current track of the focused deck (keyboard shortcut;
    /// the subscription has no access to the focus).
    DeleteFocusedTrack,
    /// Highlight a catalog row.
    SelectCatalogTrack(usize),
    /// Pause every deck the current layout does not lay out.
    PauseHiddenDecks,
    /// Periodic tick from the subscription.
    Tick,
    /// Window chrome the bar draws itself; executed against the
    /// window this app owns.
    Window(WindowCommand),
    /// The window settled at a new size; the menu draws it.
    WindowResized(iced::Size),
    /// The window manager asked the window to close; exits the app.
    WindowCloseRequested,
}
