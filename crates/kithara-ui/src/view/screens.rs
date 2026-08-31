use std::mem;

use crate::{compile::CompiledUi, view::ViewState};

/// Whether one compiled screen is the one this view stands at.
///
/// A screen answers for the pages it showed. A state standing nowhere asks for
/// the page its `Tabs` calls initial, which is the same screen, so a document
/// just mounted and one turned back to its first page are one screen rather
/// than two.
fn fits(view: &ViewState, ui: &CompiledUi) -> bool {
    let views = ui.views();
    views
        .pages()
        .iter()
        .all(|(state, at)| views.standing(view, state) == Some(at.shown.as_str()))
}

/// The compiled screens one host keeps while its document turns between pages.
///
/// A page names a document of its own, and an [`crate::ids::InternId`] is valid
/// only inside the compiled screen that made it, so what is kept is a whole
/// screen rather than a fragment to splice into another tree. The screen being
/// shown is one of them: turning to a page already visited costs nothing, and
/// the least recently shown is dropped once the configured depth is full.
pub struct Screens {
    kept: Vec<CompiledUi>,
    limit: usize,
    shown: CompiledUi,
}

impl Screens {
    /// Keeps one compiled screen, as the one a host is showing.
    #[must_use]
    pub const fn new(limit: usize, shown: CompiledUi) -> Self {
        Self {
            kept: Vec::new(),
            limit,
            shown,
        }
    }

    /// The screen a host is showing.
    #[must_use]
    pub const fn shown(&self) -> &CompiledUi {
        &self.shown
    }

    /// Shows the screen this view stands at, compiling it only when no screen
    /// kept already shows those pages, and answers whether the screen changed.
    ///
    /// # Errors
    /// Returns whatever compiling the screen returned.
    pub fn show<E, Build>(&mut self, view: &ViewState, build: Build) -> Result<bool, E>
    where
        Build: FnOnce() -> Result<CompiledUi, E>,
    {
        if fits(view, &self.shown) {
            return Ok(false);
        }
        let ui = match self.kept.iter().position(|kept| fits(view, kept)) {
            Some(index) => self.kept.remove(index),
            None => build()?,
        };
        self.kept.push(mem::replace(&mut self.shown, ui));
        while self.kept.len() > self.limit.saturating_sub(1) {
            self.kept.remove(0);
        }
        Ok(true)
    }

    /// Shows one screen and drops every page kept for the one before it.
    ///
    /// A screen kept answers for the document and skin it was compiled
    /// against. Another document, or another skin, measures its pages
    /// differently, so what was kept answers for nothing.
    pub fn reset(&mut self, shown: CompiledUi) {
        self.kept.clear();
        self.shown = shown;
    }
}
