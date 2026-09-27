mod descent;
mod lifecycle;
mod mutation;
mod pending;
mod placement;
mod preparation;
mod relocation;
mod state;
#[cfg(test)]
pub(crate) mod tests;
mod timeline;
mod transaction;

pub use descent::SyncStaged;
pub(crate) use placement::entry_earliest;
pub use state::GroupState;
