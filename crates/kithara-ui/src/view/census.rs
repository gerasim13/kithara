use std::collections::{BTreeMap, BTreeSet};

use crate::{
    error::UiDocError,
    expand::ControlSite,
    ids::SourceUri,
    module::{BindingRef, ControlNode, ViewSet},
    view::ViewState,
};

/// What one press writes into the state it names.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ViewWrite<'a> {
    Flag(ViewSet),
    Page(&'a str),
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Write {
    Flag(ViewSet),
    Page(String),
}

impl<'a> From<&'a Write> for ViewWrite<'a> {
    fn from(write: &'a Write) -> Self {
        match write {
            Write::Flag(set) => Self::Flag(*set),
            Write::Page(page) => Self::Page(page),
        }
    }
}

/// Where one page-turning state stood when a screen was compiled.
///
/// A screen shows the page its state stands at, and the document's own initial
/// page while it stands at none. Both are kept: a host looking for a screen it
/// already compiled has only the state to go by, and a state standing nowhere
/// asks for the same screen as one standing at the initial page.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct PageStanding {
    pub initial: String,
    pub shown: String,
}

/// Where each press writes, by the path of the control that publishes it.
///
/// A host draining a press looks it up here before the application is told, so
/// a document turning its own state needs no application code to do it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ViewWrites {
    by_path: BTreeMap<String, (String, Write)>,
    named: BTreeSet<String>,
    pages: BTreeMap<String, PageStanding>,
}

impl ViewWrites {
    /// What the press at `path` writes, or nothing when it writes no state.
    #[must_use]
    pub fn at(&self, path: &str) -> Option<(&str, ViewWrite<'_>)> {
        self.by_path
            .get(path)
            .map(|(state, write)| (state.as_str(), write.into()))
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

    /// Where every page-turning state stood when this screen was compiled.
    #[must_use]
    pub const fn pages(&self) -> &BTreeMap<String, PageStanding> {
        &self.pages
    }

    /// The page one state stands at on this screen: the page the view was
    /// turned to, or the one the document calls initial while it has been
    /// turned nowhere.
    #[must_use]
    pub fn standing<'a>(&'a self, view: &'a ViewState, state: &str) -> Option<&'a str> {
        let at = self.pages.get(state)?;
        Some(view.page(state).unwrap_or(&at.initial))
    }
}

/// One `Tabs` as it compiled: the pages it offers and the one it showed.
pub(crate) struct Tabs<'a> {
    pub(crate) initial: &'a str,
    pub(crate) origin: &'a SourceUri,
    pub(crate) pages: BTreeSet<String>,
    pub(crate) path: &'a str,
    pub(crate) shown: &'a str,
    pub(crate) state: &'a str,
}

/// Which way one binding runs, which is the slot it fills rather than anything
/// the binding itself says.
#[derive(Clone, Copy)]
pub(crate) enum Side {
    Read,
    Write,
}

/// One naming of a page, kept until the pages a `Tabs` declares are known.
struct Named {
    origin: SourceUri,
    page: String,
    path: String,
    state: String,
}

/// What one document says about the states it names, gathered while it expands.
///
/// A state the document writes and never reads is a name nothing shows, which
/// is a typo rather than a screen: a misspelt name on either side leaves the
/// one it was meant to be unwritten and the one it became unread. A state only
/// read is left alone, because an application is allowed to be the only thing
/// that moves it.
#[derive(Default)]
pub(crate) struct Census {
    declared: BTreeMap<String, BTreeSet<String>>,
    named: Vec<Named>,
    origin: BTreeMap<String, (SourceUri, String)>,
    pages: BTreeMap<String, PageStanding>,
    read: BTreeSet<String>,
    writes: BTreeMap<String, (String, Write)>,
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
        let (state, write) = match binding {
            BindingRef::View { id, set } => (&id.0, Write::Flag(*set)),
            BindingRef::Page { id, name } => {
                self.named.push(Named {
                    origin: origin.clone(),
                    page: name.clone(),
                    path: path.to_owned(),
                    state: id.0.clone(),
                });
                (&id.0, Write::Page(name.clone()))
            }
            BindingRef::Command { .. }
            | BindingRef::Model { .. }
            | BindingRef::Parameter { .. }
            | BindingRef::Telemetry { .. } => return,
        };
        match side {
            Side::Read => {
                self.read.insert(state.clone());
            }
            Side::Write => {
                self.writes.insert(path.to_owned(), (state.clone(), write));
                self.origin
                    .entry(state.clone())
                    .or_insert_with(|| (origin.clone(), path.to_owned()));
            }
        }
    }

    /// Notes the pages one `Tabs` offers, which of them it showed, and that it
    /// reads the state naming which of them stands.
    pub(crate) fn note_pages(&mut self, tabs: Tabs<'_>) {
        self.read.insert(tabs.state.to_owned());
        self.declared
            .entry(tabs.state.to_owned())
            .or_default()
            .extend(tabs.pages);
        self.origin
            .entry(tabs.state.to_owned())
            .or_insert_with(|| (tabs.origin.clone(), tabs.path.to_owned()));
        self.pages.insert(
            tabs.state.to_owned(),
            PageStanding {
                initial: tabs.initial.to_owned(),
                shown: tabs.shown.to_owned(),
            },
        );
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
            self.writes.insert(
                site.path.to_owned(),
                (id.0.clone(), Write::Flag(ViewSet::Off)),
            );
        }
    }

    pub(crate) fn finish(self) -> Result<ViewWrites, UiDocError> {
        if let Some(named) = self.named.iter().find(|named| {
            !self
                .declared
                .get(&named.state)
                .is_some_and(|pages| pages.contains(&named.page))
        }) {
            return Err(UiDocError::UnknownPage {
                origin: named.origin.clone(),
                id: named.state.clone(),
                page: named.page.clone(),
                path: named.path.clone(),
            });
        }
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
            pages: self.pages,
        })
    }
}
