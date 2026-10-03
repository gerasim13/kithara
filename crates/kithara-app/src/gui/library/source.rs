use std::collections::BTreeMap;

use kithara::ui::{
    error::UiDocError,
    ids::{NodeId, SourceUri},
    module::{ControlNode, IconName},
    render::TableRow,
    text::TextDoc,
};

use super::PagesModule;

pub(in crate::gui) mod consts {
    /// The page the built-in sources draw, relative to the package root.
    pub(in crate::gui) const SOURCE_PAGE: &str = "modules/library/source-page.kmodule.ron";
}

/// A source's id and page module, known before the source is built.
#[derive(Clone, Debug)]
pub(in crate::gui) struct SourcePage {
    pub(in crate::gui) id: &'static str,
    pub(in crate::gui) page: ControlNode,
}

impl SourcePage {
    pub(in crate::gui) fn table(id: &'static str) -> Self {
        Self {
            id,
            page: ControlNode::Include {
                id: NodeId(format!("{id}-page")),
                source: consts::SOURCE_PAGE.to_owned(),
                with: BTreeMap::from([("source".to_owned(), id.to_owned())]),
            },
        }
    }
}

/// Builds a registered source from the package's text catalog.
type Build = Box<dyn FnOnce(&TextDoc) -> Result<Box<dyn LibrarySource>, UiDocError>>;

/// A source to mount: its page and how to build it from the text catalog.
#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in)]
pub(in crate::gui) struct Registration {
    #[field(get, vis = "pub(in crate::gui)")]
    page: SourcePage,
    build: Build,
}

impl Registration {
    pub(in crate::gui) fn new(
        page: SourcePage,
        build: impl FnOnce(&TextDoc) -> Result<Box<dyn LibrarySource>, UiDocError> + 'static,
    ) -> Self {
        Self {
            page,
            build: Box::new(build),
        }
    }

    pub(in crate::gui) fn build(
        self,
        text: &TextDoc,
    ) -> Result<Box<dyn LibrarySource>, UiDocError> {
        (self.build)(text)
    }
}

/// One branch of the library tree and the page its nodes show.
pub(in crate::gui) trait LibrarySource {
    fn analysis_key(&self, row: usize) -> Option<&str>;

    fn branch(&self) -> &BranchNode;

    fn expand(&mut self, node: &str);

    fn id(&self) -> &str;

    fn rows(&self, selected: Option<&str>) -> Vec<TableRow<'_>>;

    fn row_key(&self, row: usize) -> Option<&str>;

    fn select(&mut self, node: &str);

    fn status(&self) -> PageStatus;

    fn tick(&mut self);
}

/// Where a source's page stands; the shell words it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::gui) enum PageStatus {
    /// It lists at least one row.
    Ready,
    Loading,
    /// It has no row to list.
    Empty,
    Unreadable,
}

/// One node of a source's branch.
pub(in crate::gui) struct BranchNode {
    /// Names the node to its source; unique within the branch.
    pub(in crate::gui) key: String,
    pub(in crate::gui) label: String,
    pub(in crate::gui) icon: IconName,
    pub(in crate::gui) count: Option<u32>,
    pub(in crate::gui) children: Vec<Self>,
    /// Its children are not known yet; it opens all the same.
    pub(in crate::gui) unlisted: bool,
}

impl BranchNode {
    pub(in crate::gui) fn new(key: &str, label: String, icon: IconName) -> Self {
        Self {
            label,
            icon,
            key: key.to_owned(),
            count: None,
            children: Vec::new(),
            unlisted: false,
        }
    }
}

pub(in crate::gui) fn worded(text: &TextDoc, key: &str, path: &str) -> Result<String, UiDocError> {
    text.get(key)
        .map(str::to_owned)
        .ok_or_else(|| UiDocError::UnknownTextKey {
            origin: SourceUri(PagesModule::PATH.to_owned()),
            key: key.to_owned(),
            path: path.to_owned(),
        })
}
