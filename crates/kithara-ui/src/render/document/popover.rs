use crate::{
    expand::Binding,
    ids::InternId,
    module::{PopoverAlign, PopoverAt},
    size::SizeSpec,
};

/// Resolved toolkit-neutral placement of one document popover.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct Popover<'a> {
    pub(super) flag: &'a Binding,
    pub(super) path: InternId,
    pub(super) size: Option<SizeSpec>,
    pub(super) align: PopoverAlign,
    pub(super) at: PopoverAt,
    pub(super) open: bool,
}

impl<'a> Popover<'a> {
    /// Edge alignment against the opening geometry.
    #[must_use]
    pub const fn align(&self) -> PopoverAlign {
        self.align
    }

    /// Geometry the overlay opens from.
    #[must_use]
    pub const fn at(&self) -> PopoverAt {
        self.at
    }

    /// What [`Self::is_open`] was read from.
    ///
    /// A host that rebuilds its tree every frame is handed the answer above and
    /// needs no more. One that mounts a tree and keeps it has to read the flag
    /// again when the document is shown again, because the surface opening is
    /// not a value inside the content — it is the content being there at all.
    #[must_use]
    pub const fn flag(&self) -> &'a Binding {
        self.flag
    }

    /// Whether the document holds the overlay open right now.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.open
    }

    /// Event path published by the anchor.
    #[must_use]
    pub const fn path(&self) -> InternId {
        self.path
    }

    /// Effective in-flow size, inherited from the anchor.
    #[must_use]
    pub const fn size(&self) -> Option<SizeSpec> {
        self.size
    }
}
