use std::collections::{BTreeMap, BTreeSet};

use crate::{module::ViewSet, view::ViewWrite};

/// A screen that has turned nothing, for a reader that keeps no state of its
/// own to hand a document.
pub static EMPTY: ViewState = ViewState::new();

/// Where one state stands.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Stands {
    Flag(bool),
    Page(String),
}

/// State a screen keeps for itself, which no application declares, answers, or
/// is told about.
///
/// A state lives under the name the document gave it inside the module instance
/// that named it, so two includes of one module keep two states without the
/// document saying so. What is not in the map has never been written: a flag
/// reads false, and a page-turning state stands at whichever page its `Tabs`
/// calls initial.
#[derive(Clone, Debug, Default)]
pub struct ViewState {
    stands: BTreeMap<String, Stands>,
}

impl ViewState {
    /// A screen that has turned nothing yet.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            stands: BTreeMap::new(),
        }
    }

    /// Applies whatever one press writes, and answers whether it moved the
    /// state.
    pub fn apply(&mut self, state: &str, write: ViewWrite<'_>) -> bool {
        match write {
            ViewWrite::Flag(set) => self.set(state, set),
            ViewWrite::Page(page) => self.stand(state, page),
        }
    }

    #[must_use]
    pub fn flag(&self, state: &str) -> bool {
        matches!(self.stands.get(state), Some(Stands::Flag(true)))
    }

    /// The page this state stands at, or nothing while it has been turned to
    /// none and the document's own initial page still answers.
    #[must_use]
    pub fn page(&self, state: &str) -> Option<&str> {
        match self.stands.get(state) {
            Some(Stands::Page(page)) => Some(page),
            Some(Stands::Flag(_)) | None => None,
        }
    }

    /// Drops every state the screen now being shown does not name.
    ///
    /// A state belongs to the document that declared it. Another document is
    /// another declaration, so what it does not name is gone rather than
    /// carried over to answer for a state that no longer exists.
    pub fn retain(&mut self, named: &BTreeSet<String>) {
        self.stands.retain(|state, _| named.contains(state));
    }

    /// Applies one write and answers whether it moved the flag.
    pub fn set(&mut self, state: &str, set: ViewSet) -> bool {
        let was = self.flag(state);
        let now = match set {
            ViewSet::Toggle => !was,
            ViewSet::On => true,
            ViewSet::Off => false,
        };
        if now == was {
            return false;
        }
        self.stands.insert(state.to_owned(), Stands::Flag(now));
        true
    }

    /// Stands the state at one page, and answers whether it moved.
    pub fn stand(&mut self, state: &str, page: &str) -> bool {
        if self.page(state) == Some(page) {
            return false;
        }
        self.stands
            .insert(state.to_owned(), Stands::Page(page.to_owned()));
        true
    }

    /// Every page a screen stands at, which is the whole of what decides the
    /// shape its document compiles to.
    #[must_use]
    pub fn standing(&self) -> BTreeMap<&str, &str> {
        self.stands
            .iter()
            .filter_map(|(state, stands)| match stands {
                Stands::Page(page) => Some((state.as_str(), page.as_str())),
                Stands::Flag(_) => None,
            })
            .collect()
    }
}
