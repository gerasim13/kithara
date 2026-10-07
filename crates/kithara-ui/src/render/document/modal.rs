use crate::{expand::Binding, ids::InternId};

/// One document modal, resolved for the frame being mounted.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct Modal<'a> {
    pub(super) flag: &'a Binding,
    pub(super) path: InternId,
    pub(super) open: bool,
}

impl<'a> Modal<'a> {
    /// What [`Self::is_open`] was read from, for a host that keeps its tree
    /// and reads the flag again on every refresh.
    #[must_use]
    pub const fn flag(&self) -> &'a Binding {
        self.flag
    }

    /// Whether the document holds the modal open right now.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.open
    }

    /// The path Escape and a press on the scrim publish on, which the
    /// document routes to the modal's close binding.
    #[must_use]
    pub const fn path(&self) -> InternId {
        self.path
    }
}
