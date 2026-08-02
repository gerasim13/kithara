pub(super) mod cache;
mod compile;
pub(super) mod endpoints;
mod events;
pub(super) mod scope;
mod shortcut;
#[cfg(test)]
mod tests;

pub(super) use shortcut::deletes_focused_track;

pub(crate) use self::{
    compile::{StudioUi, view},
    events::translate,
};
