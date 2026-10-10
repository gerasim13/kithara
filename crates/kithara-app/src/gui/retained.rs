use std::rc::Rc;

use iced::{Size, window::Id};
use kithara::ui::{
    app,
    app::{App, Config, RunError},
    render::{Reads, Skin, UiEvent, Walk},
    source::UiConfig,
};
use num_traits::cast::AsPrimitive;

use super::{
    app::Kithara,
    frontend::Boot,
    message::Message,
    reads::ReadRoot,
    ui::{self, window::consts::WINDOW_SIZE},
    update,
};

/// The studio driven by the retained host.
///
/// Everything below the surface is the studio the immediate host shows: the
/// same state, the same reads, the same translation of what the document
/// published. Only the shell differs.
pub(crate) struct Studio {
    state: Kithara,
    settings: UiConfig,
}

impl Studio {
    pub(crate) fn new(boot: Boot) -> Self {
        Self {
            settings: boot.settings.clone(),
            state: Kithara::mounted(boot, Id::unique()),
        }
    }
}

impl App for Studio {
    fn document(&self) -> &str {
        self.state.ui.package.document(self.state.ui.cache.layout())
    }

    /// The studio answers its endpoints by walking its own state, so the walk
    /// borrows the studio and lives no longer than this call.
    fn reads<R>(&self, with: impl FnOnce(&dyn Reads) -> R) -> R {
        let root = ReadRoot::new(&self.state);
        with(&Walk::new(&root))
    }

    fn skin(&self) -> &Skin {
        self.state.ui.package.skin()
    }

    fn tick(&mut self) {
        drop(update::update(&mut self.state, Message::Tick));
    }

    fn update(&mut self, event: UiEvent) {
        if matches!(event, UiEvent::Window(_)) {
            return;
        }
        if let Some(message) = ui::translate(&mut self.state, event) {
            drop(update::update(&mut self.state, message));
        }
    }
}

/// Opens the studio window and runs it under the retained host.
///
/// # Errors
/// Returns [`RunError`] when the studio document does not compile, or when the
/// window and its GPU surface cannot be brought up.
pub(crate) fn run(app: Studio) -> Result<(), RunError> {
    let package = Rc::clone(&app.state.ui.package);
    let (size, min_size) = (window_size(), window_min(app.state.ui.window_min()));
    let settings = app.settings.clone();
    app::run(
        app,
        Config::builder()
            .endpoints(package.registry())
            .resolver(package.resolver())
            .text(package.text())
            .decorations(false)
            .min_size(min_size)
            .settings(&settings)
            .title("Kithara - DJ Studio")
            .build(),
        size,
    )
}

pub(crate) fn window_size() -> (u32, u32) {
    (whole(WINDOW_SIZE.width), whole(WINDOW_SIZE.height))
}

fn window_min(min: Size) -> (u32, u32) {
    (whole(min.width), whole(min.height))
}

fn whole(value: f32) -> u32 {
    value.as_()
}

#[cfg(test)]
mod tests {
    use ::kithara::ui::{
        app::Ui,
        render::{ReadValue, Reads},
    };
    use kithara_test_utils::kithara;

    use super::{App, Config, Rc, Skin, ui, ui::package::Package};

    /// A studio with nothing loaded: every control falls back to what the
    /// document and the skin say, which is the hardest case for a host that
    /// only draws what it was told.
    struct Empty {
        package: Rc<Package>,
    }

    impl Reads for Empty {
        fn get(&self, _endpoint: &str) -> Option<ReadValue<'_>> {
            None
        }
    }

    impl App for Empty {
        fn document(&self) -> &str {
            self.package.document(ui::cache::DeckLayout::Dual)
        }

        fn reads<R>(&self, with: impl FnOnce(&dyn Reads) -> R) -> R {
            with(self)
        }

        fn skin(&self) -> &Skin {
            self.package.skin()
        }

        fn update(&mut self, _event: ::kithara::ui::render::UiEvent) {}
    }

    /// The studio's own documents draw under the retained host. The control
    /// census answers for one control at a time; this answers for the page the
    /// application actually ships, mounted the way the window mounts it.
    #[kithara::test]
    fn the_studio_draws_under_the_retained_host() {
        let package = crate::gui::test_fixture::package(None)
            .expect("the app package must answer for both decks");
        let mut ui = Ui::new(
            Empty {
                package: Rc::clone(&package),
            },
            Config::builder()
                .endpoints(package.registry())
                .resolver(package.resolver())
                .text(package.text())
                .build(),
            (1280, 760),
            1.0,
        )
        .unwrap_or_else(|error| panic!("the studio must mount under the retained host: {error}"));

        let frame = ui
            .render()
            .unwrap_or_else(|error| panic!("the studio must reach a paint pass: {error}"));

        let encoding = frame.scene().encoding();

        assert!(
            !encoding.is_empty(),
            "the retained host mounted the studio and drew nothing"
        );
        assert!(
            !encoding.resources.glyphs.is_empty(),
            "the studio is a page of labelled controls; a scene with no glyphs in it is a \
             background and nothing else"
        );
    }
}

#[cfg(test)]
mod library {
    use ::kithara::ui::app::Ui;
    use kithara_test_utils::kithara;

    use super::{Studio, window_size};
    use crate::gui::test_fixture::{
        self,
        retained::{click, laid, studio, tree_row},
    };

    fn mounted(check: impl FnOnce(&mut Ui<'_, Studio>)) {
        studio(Vec::new(), window_size(), check);
    }

    fn shown(ui: &mut Ui<'_, Studio>) -> Vec<&'static str> {
        ["startup", "explorer"]
            .into_iter()
            .filter(|source| laid(ui, &format!("library/pages/{source}/rows")).is_some())
            .collect()
    }

    fn labels(ui: &Ui<'_, Studio>) -> Vec<String> {
        ui.app()
            .state
            .library
            .tree()
            .iter()
            .map(|row| row.label.to_owned())
            .collect()
    }

    #[kithara::test(native, flash(false))]
    fn the_page_on_screen_is_the_selected_sources() {
        mounted(|ui| {
            assert_eq!(shown(ui), ["startup"], "Startup starts selected");

            let explorer = tree_row(ui, 3);
            click(ui, explorer);
            assert_eq!(shown(ui), ["explorer"]);

            let startup = tree_row(ui, 1);
            click(ui, startup);
            assert_eq!(shown(ui), ["startup"]);
        });
    }

    #[kithara::test(native, flash(false))]
    fn a_plugin_page_stands_when_its_source_is_selected() {
        let (probe, _) = test_fixture::Probe::registered("menu.module.library");
        studio(vec![probe], window_size(), |ui| {
            let page = "library/pages/probe/rows";
            assert!(laid(ui, page).is_none(), "Startup starts selected");

            let label = labels(ui)
                .iter()
                .position(|label| label == "leaf")
                .unwrap_or_else(|| panic!("the plugin is listed: {:?}", labels(ui)));
            let at = tree_row(ui, u8::try_from(label).unwrap_or(u8::MAX));
            click(ui, at);

            assert_eq!(ui.app().state.library.page(), Some("probe"));
            assert!(laid(ui, page).is_some(), "the plugin page stands");
            assert_eq!(shown(ui), Vec::<&str>::new());
        });
    }

    #[kithara::test(native, flash(false))]
    fn library_followup_section_label_toggles_without_selecting() {
        mounted(|ui| {
            let closed = labels(ui);
            let chevron = tree_row(ui, 0);

            click(ui, chevron);
            assert_eq!(labels(ui).len(), closed.len() - 1);
            assert_eq!(shown(ui), ["startup"]);

            click(ui, chevron);
            assert_eq!(labels(ui), closed);
        });
    }
}
