mod graph;
mod modules;
mod resolve;

pub(super) use graph::{DeclarationIndex, DeclarationKey};
pub(super) use modules::known_attrs;

#[cfg(test)]
mod tests;
