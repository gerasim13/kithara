use bon::Builder;

use crate::expand::Binding;

/// The library tree, with a search field when it reads or writes a query.
#[derive(Builder, kithara_derive::Control)]
#[control(size = skin.tree.size)]
pub(crate) struct Tree<'a> {
    pub(crate) query: Option<&'a Binding>,
    pub(crate) search: bool,
    /// Whether a pressed chevron writes apart from its row.
    pub(crate) toggle: bool,
}
