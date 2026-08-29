use super::fixture;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Tab {
    Atoms,
    Buttons,
    Faders,
    Modules,
    Typography,
    Cells,
    Sizes,
    Tokens,
    Micro,
    Mixer,
    Vis,
    Chrome,
    Titlebars,
    Table,
    Tree,
    Library2,
    Stress,
    Menu,
    Clock,
    Pivot,
    Shader,
    Objects,
    Motion,
    Sprites,
    Lottie,
    Scene,
    TableLong,
    Custom,
    Skins,
}

impl Tab {
    /// Every page the gallery offers, in the order the nav lists them, each
    /// with the slug its nav item and its document share: the item a press
    /// arrives from stands at `gallery/<slug>/item`, and the package answers
    /// for the page as `gallery-<slug>`.
    ///
    /// One list stands behind all three, so a page is added by writing it here
    /// and in the documents, and nowhere else.
    const PAGES: [(Self, &'static str); 29] = [
        (Self::Atoms, "atoms"),
        (Self::Buttons, "buttons"),
        (Self::Faders, "faders"),
        (Self::Modules, "modules"),
        (Self::Typography, "typography"),
        (Self::Cells, "cells"),
        (Self::Sizes, "sizes"),
        (Self::Tokens, "tokens"),
        (Self::Micro, "micro"),
        (Self::Mixer, "mixer"),
        (Self::Vis, "vis"),
        (Self::Chrome, "chrome"),
        (Self::Titlebars, "titlebars"),
        (Self::Table, "table"),
        (Self::Tree, "tree"),
        (Self::Library2, "library2"),
        (Self::Stress, "stress"),
        (Self::Menu, "menu"),
        (Self::Clock, "clock"),
        (Self::Pivot, "pivot"),
        (Self::Shader, "shader"),
        (Self::Objects, "objects"),
        (Self::Motion, "motion"),
        (Self::Sprites, "sprites"),
        (Self::Lottie, "lottie"),
        (Self::Scene, "scene"),
        (Self::TableLong, "table-long"),
        (Self::Custom, "custom"),
        (Self::Skins, "skins"),
    ];

    pub(super) const ALL: [Self; Self::PAGES.len()] = {
        let mut all = [Self::Atoms; Self::PAGES.len()];
        let mut index = 0;
        while index < all.len() {
            all[index] = Self::PAGES[index].0;
            index += 1;
        }
        all
    };

    /// The page this tab shows, as the gallery's package names it: the role is
    /// the id the document states, and which file that role lives in is the
    /// manifest's to say.
    pub(super) fn entry(self) -> &'static str {
        fixture::document(&format!("gallery-{}", Self::PAGES[self.index()].1))
    }

    /// Where this tab stands among the pages, which is the order they were
    /// compiled in.
    pub(super) fn index(self) -> usize {
        Self::PAGES
            .iter()
            .position(|(tab, _)| *tab == self)
            .expect("every tab stands in the page list")
    }
}

impl TryFrom<&str> for Tab {
    type Error = ();

    fn try_from(path: &str) -> Result<Self, ()> {
        let slug = path
            .strip_prefix("gallery/")
            .and_then(|rest| rest.strip_suffix("/item"))
            .ok_or(())?;
        Self::PAGES
            .iter()
            .find(|(_, named)| *named == slug)
            .map(|(tab, _)| *tab)
            .ok_or(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ModuleDemo {
    Deck,
    DeckMicro,
    GlobalBar,
    Telemetry,
    Layout,
}

impl ModuleDemo {
    /// The module pages, in the order the demo offers them, each with the role
    /// the package answers for it.
    const PAGES: [(Self, &'static str); 5] = [
        (Self::Deck, "gallery-modules"),
        (Self::DeckMicro, "gallery-modules-deck-micro"),
        (Self::GlobalBar, "gallery-modules-global-bar"),
        (Self::Telemetry, "gallery-modules-telemetry"),
        (Self::Layout, "gallery-modules-layout"),
    ];

    pub(super) const ALL: [Self; Self::PAGES.len()] = {
        let mut all = [Self::Deck; Self::PAGES.len()];
        let mut index = 0;
        while index < all.len() {
            all[index] = Self::PAGES[index].0;
            index += 1;
        }
        all
    };

    pub(super) fn entry(self) -> &'static str {
        fixture::document(Self::PAGES[self.index()].1)
    }

    pub(super) fn index(self) -> usize {
        Self::PAGES
            .iter()
            .position(|(demo, _)| *demo == self)
            .expect("every module page stands in the list")
    }
}
