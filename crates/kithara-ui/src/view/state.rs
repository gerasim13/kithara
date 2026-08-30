use std::collections::{BTreeMap, BTreeSet};

use crate::module::ViewSet;

/// A screen that has turned nothing, for a reader that keeps no state of its
/// own to hand a document.
pub static EMPTY: ViewState = ViewState::new();

/// State a screen keeps for itself, which no application declares, answers, or
/// is told about.
///
/// A flag lives under the name the document gave it inside the module instance
/// that named it, so two includes of one module keep two flags without the
/// document saying so. What is not in the map has never been written and reads
/// false.
#[derive(Clone, Debug, Default)]
pub struct ViewState {
    flags: BTreeMap<String, bool>,
}

impl ViewState {
    /// A screen that has turned nothing yet.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            flags: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn flag(&self, state: &str) -> bool {
        self.flags.get(state).copied().unwrap_or_default()
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
        self.flags.insert(state.to_owned(), now);
        true
    }

    /// Drops every flag whose state the screen now being shown does not name.
    ///
    /// A flag belongs to the document that declared it. Another document is
    /// another declaration, so what it does not name is gone rather than
    /// carried over to answer for a state that no longer exists.
    pub fn retain(&mut self, named: &BTreeSet<String>) {
        self.flags.retain(|state, _| named.contains(state));
    }
}
