use crate::size::{Dim, SizeSpec};

/// A search field backed by the shared text-input engine.
#[derive(kithara_derive::Control)]
#[control(size = SizeSpec::new(Dim::Fill, Dim::Fixed(skin.tree.search_height)))]
pub(crate) struct Search;
