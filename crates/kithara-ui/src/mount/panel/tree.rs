use bon::Builder;

#[cfg(any(feature = "iced", feature = "masonry"))]
use crate::expand::Binding;

/// The library tree, with a search field when it reads or writes a query.
#[derive(Builder, kithara_derive::Control)]
#[control(size = skin.tree.size)]
pub(crate) struct Tree<#[cfg(any(feature = "iced", feature = "masonry"))] 'a> {
    #[cfg(any(feature = "iced", feature = "masonry"))]
    pub(crate) query: Option<&'a Binding>,
    #[cfg(any(feature = "iced", feature = "masonry"))]
    pub(crate) search: bool,
    /// Whether a pressed chevron writes apart from its row.
    #[cfg(any(feature = "iced", feature = "masonry"))]
    pub(crate) toggle: bool,
}
