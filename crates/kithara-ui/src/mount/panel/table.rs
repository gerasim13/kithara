use bon::Builder;

use crate::{
    expand::Binding,
    module::{TableColumn, TableFrame},
};

/// A table whose columns and row values are supplied by the document and host.
#[derive(Builder, kithara_derive::Control)]
#[control(size = skin.table.size)]
pub(crate) struct Table<'a> {
    pub(crate) columns: &'a [TableColumn],
    pub(crate) columns_state: Option<&'a Binding>,
    pub(crate) width: Option<&'a Binding>,
    pub(crate) frame: TableFrame,
    pub(crate) status: Option<&'a Binding>,
}
