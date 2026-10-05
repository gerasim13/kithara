use kithara_ui::{
    error::UiDocError,
    registry::{EndpointCategory, ValueKind},
    text::TextDoc,
};

use crate::LibrarySource;

/// A document a source brings into the package.
#[derive(Clone, Copy)]
pub struct Document {
    /// The package-relative path the document is named by.
    pub path: &'static str,
    pub text: &'static str,
}

/// A source's id and page, known before the source is built.
pub struct SourcePage {
    /// Names the source; its page reads and writes are scoped by it.
    pub id: &'static str,
    /// The module the shell mounts as the source's page, handing it the id as
    /// its `source` parameter.
    pub page: &'static str,
    /// Reads and writes the source's page declares.
    pub endpoints: Vec<Endpoint>,
    /// Modules the source brings into the package.
    pub modules: Vec<Document>,
    /// Caption catalogs laid over the package's own.
    pub texts: Vec<Document>,
}

/// A read or write a source's page declares. The shell registers it as
/// `source.<name>` scoped by `source` and routes it to the scoped source.
#[derive(Clone, Copy)]
pub struct Endpoint {
    /// One segment, distinct from the shell's own `rows`, `status`, `select`
    /// and `column`.
    pub name: &'static str,
    pub category: EndpointCategory,
    pub value: ValueKind,
}

impl SourcePage {
    /// A source the shell's table page draws.
    #[must_use]
    pub const fn table(id: &'static str) -> Self {
        Self {
            id,
            page: "modules/library/source-page.kmodule.ron",
            endpoints: Vec::new(),
            modules: Vec::new(),
            texts: Vec::new(),
        }
    }
}

/// Builds a registered source from the package's text catalog.
type Build = Box<dyn FnOnce(&TextDoc) -> Result<Box<dyn LibrarySource>, UiDocError>>;

/// A source to mount: its page and how to build it from the text catalog.
pub struct Registration {
    build: Build,
    page: SourcePage,
}

impl Registration {
    pub fn new<F>(page: SourcePage, build: F) -> Self
    where
        F: FnOnce(&TextDoc) -> Result<Box<dyn LibrarySource>, UiDocError> + 'static,
    {
        Self {
            page,
            build: Box::new(build),
        }
    }

    /// Builds the source once the package's text catalog is known.
    ///
    /// # Errors
    /// Returns the error of a label the catalog does not word.
    pub fn build(self, text: &TextDoc) -> Result<Box<dyn LibrarySource>, UiDocError> {
        (self.build)(text)
    }

    #[must_use]
    pub const fn page(&self) -> &SourcePage {
        &self.page
    }
}
