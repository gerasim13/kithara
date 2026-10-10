mod assignments;
mod index;
mod literals;

pub(super) use assignments::crate_name_from_rel;
#[cfg(test)]
pub(super) use index::build_index_from_source;
pub(super) use index::{LiteralSite, WorkspaceStructIndex, build_index};
pub(super) use literals::{full_literal_sites, unique_consumer};
