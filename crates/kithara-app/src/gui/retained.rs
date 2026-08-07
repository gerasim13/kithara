use iced::window;
use kithara_ui::{
    app::{App, Config, RunError},
    builtin,
    render::{Reads, UiEvent, Walk},
};

use super::{
    app::{Decks, Kithara},
    frontend::studio_size,
    message::Message,
    studio_reads::StudioRoot,
    studio_ui::{self, StudioUi},
    update,
};
use crate::{catalog::Catalog, config::AppConfig, deck::DeckSet, gui::studio_ui::endpoints};

/// The studio driven by the retained host.
///
/// Everything below the surface is the studio the immediate host shows: the
/// same state, the same reads, the same translation of what the document
/// published. Only the shell differs.
pub(crate) struct Studio {
    state: Kithara,
}

impl Studio {
    pub(crate) fn new(
        session: DeckSet,
        decks: Decks,
        catalog: Catalog,
        config: AppConfig,
        studio: StudioUi,
        palette: crate::theme::gui::GuiPalette,
    ) -> Self {
        Self {
            state: Kithara::mounted(
                session,
                decks,
                catalog,
                config,
                studio,
                palette,
                window::Id::unique(),
            ),
        }
    }
}

impl App for Studio {
    fn document(&self) -> &str {
        studio_ui::entry(self.state.studio.cache.layout())
    }

    /// The studio answers its endpoints by walking its own state, so the walk
    /// borrows the studio and lives no longer than this call.
    fn reads<R>(&self, with: impl FnOnce(&dyn Reads) -> R) -> R {
        let root = StudioRoot::new(&self.state);
        with(&Walk::new(&root))
    }

    fn update(&mut self, event: UiEvent) {
        // A window command is the host's to carry out, and this host takes it
        // from `take_window_commands` instead.
        if matches!(event, UiEvent::Window(_)) {
            return;
        }
        if let Some(message) = studio_ui::translate(&mut self.state, event) {
            drop(update::update(&mut self.state, message));
        }
    }

    fn tick(&mut self) {
        drop(update::update(&mut self.state, Message::Tick));
    }
}

/// Opens the studio window and runs it under the retained host.
///
/// # Errors
/// Returns [`RunError`] when the studio document does not compile, or when the
/// window and its GPU surface cannot be brought up.
pub(crate) fn run(app: Studio) -> Result<(), RunError> {
    let resolver = studio_ui::resolver();
    let endpoints = endpoints::StudioRegistry::default();
    let (size, min_size) = studio_size();
    kithara_ui::app::run(
        app,
        Config::builder()
            .endpoints(&endpoints)
            .resolver(&resolver)
            .skin(builtin::skin())
            .skin_doc(builtin::skin_doc())
            .decorations(false)
            .min_size(min_size)
            .title("Kithara - DJ Studio")
            .build(),
        size,
    )
}
