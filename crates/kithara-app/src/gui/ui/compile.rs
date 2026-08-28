use iced::{Element, Size};
use kithara_platform::time::Duration;
use kithara_ui::{
    builtin,
    compile::{CompiledUi, compile},
    error::UiDocError,
    ids::{ScreenRole, SourceUri},
    package::load_package,
    render::{Clock, Walk, tree},
    source::{MemResolver, SourceResolver, UiConfig},
    text::{TextDoc, parse_text},
};

use super::{
    cache::{DeckLayout, ViewCache},
    endpoints::Registry,
};
use crate::gui::{app::Kithara, message::Message, reads::ReadRoot};

const DOCS: &[(&str, &str)] = &[
    (
        Screens::PACKAGE,
        include_str!("../../../assets/ui/package.kpackage.ron"),
    ),
    (
        "app.klayout.ron",
        include_str!("../../../assets/ui/app.klayout.ron"),
    ),
    (
        "app-single.klayout.ron",
        include_str!("../../../assets/ui/app-single.klayout.ron"),
    ),
    (
        "modules/app-bar.kmodule.ron",
        include_str!("../../../assets/ui/modules/app-bar.kmodule.ron"),
    ),
    (
        "modules/app-bar-micro.kmodule.ron",
        include_str!("../../../assets/ui/modules/app-bar-micro.kmodule.ron"),
    ),
    (
        "modules/app-menu.kmodule.ron",
        include_str!("../../../assets/ui/modules/app-menu.kmodule.ron"),
    ),
    (
        "modules/app-menu/window-row.kmodule.ron",
        include_str!("../../../assets/ui/modules/app-menu/window-row.kmodule.ron"),
    ),
    (
        "modules/app-menu/module-cell.kmodule.ron",
        include_str!("../../../assets/ui/modules/app-menu/module-cell.kmodule.ron"),
    ),
    (
        "modules/app-deck.kmodule.ron",
        include_str!("../../../assets/ui/modules/app-deck.kmodule.ron"),
    ),
    (
        "modules/app-overview.kmodule.ron",
        include_str!("../../../assets/ui/modules/app-overview.kmodule.ron"),
    ),
    (
        "modules/app-overview-single.kmodule.ron",
        include_str!("../../../assets/ui/modules/app-overview-single.kmodule.ron"),
    ),
    (
        "modules/app-overview-row.kmodule.ron",
        include_str!("../../../assets/ui/modules/app-overview-row.kmodule.ron"),
    ),
    (
        "modules/app-mixer.kmodule.ron",
        include_str!("../../../assets/ui/modules/app-mixer.kmodule.ron"),
    ),
    (
        "modules/app-mixer-single.kmodule.ron",
        include_str!("../../../assets/ui/modules/app-mixer-single.kmodule.ron"),
    ),
    (
        "modules/app-strip/eq-mode-row.kmodule.ron",
        include_str!("../../../assets/ui/modules/app-strip/eq-mode-row.kmodule.ron"),
    ),
    (
        "modules/app-strip.kmodule.ron",
        include_str!("../../../assets/ui/modules/app-strip.kmodule.ron"),
    ),
    (
        "modules/app-strip/eq-3-band.kmodule.ron",
        include_str!("../../../assets/ui/modules/app-strip/eq-3-band.kmodule.ron"),
    ),
    (
        "modules/app-strip/eq-4-band.kmodule.ron",
        include_str!("../../../assets/ui/modules/app-strip/eq-4-band.kmodule.ron"),
    ),
    (
        "modules/app-library.kmodule.ron",
        include_str!("../../../assets/ui/modules/app-library.kmodule.ron"),
    ),
];

/// The screen this application asks a package for, one per deck arrangement.
///
/// Both are resolved once, when the package is read, so a host that has to
/// name the document it draws can hand back a name the package already
/// answered for.
#[derive(Clone, Debug)]
pub(in crate::gui) struct Screens {
    dual: String,
    single: String,
}

impl Screens {
    /// The manifest naming the screen behind each role this application asks
    /// for.
    const PACKAGE: &'static str = "package.kpackage.ron";

    pub(in crate::gui) fn new(resolver: &dyn SourceResolver) -> Result<Self, UiDocError> {
        let package = load_package(resolver, Self::PACKAGE)?;
        Ok(Self {
            dual: package.screen(resolver, &role(DeckLayout::Dual))?,
            single: package.screen(resolver, &role(DeckLayout::Single))?,
        })
    }

    pub(in crate::gui) fn document(&self, layout: DeckLayout) -> &str {
        match layout {
            DeckLayout::Single => &self.single,
            DeckLayout::Dual => &self.dual,
        }
    }
}

/// The compiled UI plus the host-owned view state it reads back. Both
/// deck layouts are compiled once; the top bar picks which one renders.
pub(crate) struct AppUi {
    pub(crate) cache: ViewCache,
    /// This host's own reading of time, advanced once per tick so a document
    /// bound to it animates without the application keeping a timer of its own.
    clock: Clock,
    dual: CompiledUi,
    /// Only the retained host has to name the document it draws, so only that
    /// host keeps what the package answered.
    #[cfg(feature = "masonry")]
    pub(in crate::gui) screens: Screens,
    single: CompiledUi,
}

impl AppUi {
    pub(crate) fn new() -> Result<Self, UiDocError> {
        let resolver = resolver();
        let screens = Screens::new(&resolver)?;
        Ok(Self {
            single: compile_screen(&resolver, screens.document(DeckLayout::Single))?,
            dual: compile_screen(&resolver, screens.document(DeckLayout::Dual))?,
            cache: ViewCache::default(),
            clock: Clock::default(),
            #[cfg(feature = "masonry")]
            screens,
        })
    }

    /// Moves this host's clock on by one tick of `step`.
    pub(crate) fn advance(&mut self, step: Duration) {
        self.clock = self.clock.advance(step);
    }

    pub(crate) fn window_min(&self) -> Size {
        Size::new(
            self.single.min.w.min().max(self.dual.min.w.min()),
            self.single.min.h.min().max(self.dual.min.h.min()),
        )
    }

    const fn compiled(&self, layout: DeckLayout) -> &CompiledUi {
        match layout {
            DeckLayout::Single => &self.single,
            DeckLayout::Dual => &self.dual,
        }
    }
}

/// Where this application's own documents are read from: the built-in library
/// with its layouts and modules laid over it.
pub(crate) fn resolver() -> MemResolver {
    let mut resolver = builtin::resolver();
    for (path, text) in DOCS {
        resolver.insert(path, text);
    }
    resolver
}

/// What this application asks a package for, one role per deck arrangement.
///
/// The package decides which file stands behind each, so a package may rename
/// its screens without this having to know.
fn role(layout: DeckLayout) -> ScreenRole {
    ScreenRole(
        match layout {
            DeckLayout::Single => "deck-single",
            DeckLayout::Dual => "deck-dual",
        }
        .to_owned(),
    )
}

#[cfg(test)]
pub(in crate::gui) fn compile_ui(layout: DeckLayout) -> Result<CompiledUi, UiDocError> {
    let resolver = resolver();
    compile_screen(&resolver, Screens::new(&resolver)?.document(layout))
}

fn compile_screen(resolver: &MemResolver, entry: &str) -> Result<CompiledUi, UiDocError> {
    compile(
        entry,
        resolver,
        &Registry::default(),
        builtin::skin_doc(),
        &text()?,
        &UiConfig::default(),
    )
}

/// The caption catalog the documents resolve `@key` against: the built-in one
/// with this application's own entries laid over it.
pub(crate) fn text() -> Result<TextDoc, UiDocError> {
    let origin = SourceUri("app-en.ktext.ron".to_owned());
    let extra = parse_text(include_str!("../../../assets/ui/app-en.ktext.ron"), &origin)?;
    builtin::text_doc().merge(&extra, &origin)
}

pub(crate) fn view(state: &Kithara) -> Element<'_, Message> {
    let root = ReadRoot::new(state);
    let reads = Walk::new(&root);
    let compiled = state.ui.compiled(state.ui.cache.layout());
    tree::render(
        &compiled.root,
        compiled,
        &reads,
        builtin::skin(),
        state.ui.clock,
        None,
    )
    .map(Message::Ui)
}
