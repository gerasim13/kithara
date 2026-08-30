use std::collections::{BTreeMap, BTreeSet};

use crate::{
    error::UiDocError,
    expand::ControlSite,
    ids::SourceUri,
    module::{BindingRef, ControlNode, ViewSet},
};

/// Where each press writes, by the path of the control that publishes it.
///
/// A host draining a press looks it up here before the application is told, so
/// a document turning its own state needs no application code to do it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ViewWrites {
    by_path: BTreeMap<String, (String, ViewSet)>,
    named: BTreeSet<String>,
}

impl ViewWrites {
    /// What the press at `path` writes, or nothing when it writes no state.
    #[must_use]
    pub fn at(&self, path: &str) -> Option<(&str, ViewSet)> {
        self.by_path
            .get(path)
            .map(|(state, set)| (state.as_str(), *set))
    }

    /// Every state this screen names, on either side.
    #[must_use]
    pub const fn named(&self) -> &BTreeSet<String> {
        &self.named
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_path.is_empty()
    }
}

/// Which way one binding runs, which is the slot it fills rather than anything
/// the binding itself says.
#[derive(Clone, Copy)]
pub(crate) enum Side {
    Read,
    Write,
}

/// What one document says about the states it names, gathered while it expands.
///
/// A state the document writes and never reads is a name nothing shows, which
/// is a typo rather than a screen: a misspelt name on either side leaves the
/// one it was meant to be unwritten and the one it became unread. A state only
/// read is left alone, because an application is allowed to be the only thing
/// that moves it.
#[derive(Debug, Default)]
pub(crate) struct Census {
    read: BTreeSet<String>,
    writes: BTreeMap<String, (String, ViewSet)>,
    origin: BTreeMap<String, (SourceUri, String)>,
}

impl Census {
    /// Notes one binding, on the side the slot it fills puts it.
    pub(crate) fn note(
        &mut self,
        path: &str,
        binding: &BindingRef,
        origin: &SourceUri,
        side: Side,
    ) {
        let BindingRef::View { id, set } = binding else {
            return;
        };
        match side {
            Side::Read => {
                self.read.insert(id.0.clone());
            }
            Side::Write => {
                self.writes.insert(path.to_owned(), (id.0.clone(), *set));
                self.origin
                    .entry(id.0.clone())
                    .or_insert_with(|| (origin.clone(), path.to_owned()));
            }
        }
    }

    /// Notes every binding one control site carries.
    pub(crate) fn note_site(&mut self, site: ControlSite<'_>, origin: &SourceUri) {
        for binding in [
            site.read,
            site.active,
            site.columns_state,
            site.query,
            site.scope,
            site.zoom,
        ]
        .into_iter()
        .flatten()
        {
            self.note(site.path, binding, origin, Side::Read);
        }
        if let Some(binding) = site.write {
            self.note(site.path, binding, origin, Side::Write);
        }
        // A popover publishes its dismissal on its own path, so state it reads
        // for whether it stands open is state that dismissal shuts. Saying so
        // in the document would be saying twice what a popover already is.
        if let (ControlNode::Popover { .. }, Some(BindingRef::View { id, .. })) =
            (site.control, site.read)
        {
            self.writes
                .insert(site.path.to_owned(), (id.0.clone(), ViewSet::Off));
        }
    }

    pub(crate) fn finish(self) -> Result<ViewWrites, UiDocError> {
        if let Some((state, (origin, path))) = self
            .origin
            .iter()
            .find(|(state, _)| !self.read.contains(*state))
        {
            return Err(UiDocError::UnreadState {
                origin: origin.clone(),
                id: state.clone(),
                path: path.clone(),
            });
        }
        let named = self
            .read
            .iter()
            .cloned()
            .chain(self.writes.values().map(|(state, _)| state.clone()))
            .collect();
        Ok(ViewWrites {
            by_path: self.writes,
            named,
        })
    }
}
