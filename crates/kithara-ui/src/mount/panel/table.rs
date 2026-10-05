use bon::Builder;

#[cfg(any(feature = "iced", feature = "masonry"))]
use crate::{
    expand::Binding,
    module::{TableColumn, TableFrame},
};

/// A table whose columns and row values are supplied by the document and host.
#[derive(Builder, kithara_derive::Control)]
#[control(size = skin.table.size)]
pub(crate) struct Table<#[cfg(any(feature = "iced", feature = "masonry"))] 'a> {
    #[cfg(any(feature = "iced", feature = "masonry"))]
    pub(crate) columns: &'a [TableColumn],
    #[cfg(any(feature = "iced", feature = "masonry"))]
    pub(crate) columns_state: Option<&'a Binding>,
    #[cfg(any(feature = "iced", feature = "masonry"))]
    pub(crate) width: Option<&'a Binding>,
    #[cfg(any(feature = "iced", feature = "masonry"))]
    pub(crate) frame: TableFrame,
    #[cfg(any(feature = "iced", feature = "masonry"))]
    pub(crate) status: Option<&'a Binding>,
}
