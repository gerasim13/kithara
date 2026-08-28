use std::rc::Rc;

use iced::{Element, Size};
use kithara_platform::time::Duration;
use kithara_ui::{
    compile::{CompiledUi, compile},
    error::UiDocError,
    ids::SourceUri,
    render::{Clock, Walk, tree},
    source::UiConfig,
};

use super::{
    cache::{DeckLayout, ViewCache},
    endpoints::Registry,
    package::Package,
};
use crate::gui::{app::Kithara, message::Message, reads::ReadRoot};

/// The compiled UI plus the host-owned view state it reads back. Both
/// deck layouts are compiled once; the top bar picks which one renders.
pub(crate) struct AppUi {
    pub(crate) cache: ViewCache,
    /// This host's own reading of time, advanced once per tick so a document
    /// bound to it animates without the application keeping a timer of its own.
    clock: Clock,
    dual: CompiledUi,
    /// The package every page here was read, dressed and worded by. A host
    /// that has to build its own window reads it from here rather than
    /// loading a second copy.
    pub(in crate::gui) package: Rc<Package>,
    single: CompiledUi,
}

impl AppUi {
    pub(crate) fn new(package: Rc<Package>) -> Result<Self, UiDocError> {
        Ok(Self {
            single: compile_screen(&package, DeckLayout::Single)?,
            dual: compile_screen(&package, DeckLayout::Dual)?,
            cache: ViewCache::default(),
            clock: Clock::default(),
            package,
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

#[cfg(test)]
pub(in crate::gui) fn compile_ui(layout: DeckLayout) -> Result<CompiledUi, UiDocError> {
    compile_screen(Package::load(None)?.as_ref(), layout)
}

fn compile_screen(package: &Package, layout: DeckLayout) -> Result<CompiledUi, UiDocError> {
    let document = package.document(layout);
    let ui = compile(
        document,
        package.resolver(),
        &Registry::default(),
        package.skin().document(),
        package.text(),
        &UiConfig::default(),
    )?;
    ui.require_paths(Package::REQUIRED, &SourceUri(document.to_owned()))?;
    Ok(ui)
}

pub(crate) fn view(state: &Kithara) -> Element<'_, Message> {
    let root = ReadRoot::new(state);
    let reads = Walk::new(&root);
    let compiled = state.ui.compiled(state.ui.cache.layout());
    tree::render(
        &compiled.root,
        compiled,
        &reads,
        state.ui.package.skin(),
        state.ui.clock,
        None,
    )
    .map(Message::Ui)
}
