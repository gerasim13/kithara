use bon::Builder;

use crate::expand::Binding;

/// The library tree, with its own search field.
#[derive(Builder, kithara_derive::Control)]
#[control(size = skin.tree.size)]
pub(crate) struct Tree<'a> {
    pub(crate) query: Option<&'a Binding>,
}
