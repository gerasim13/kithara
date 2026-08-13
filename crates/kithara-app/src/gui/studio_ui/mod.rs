pub(super) mod cache;
mod compile;
pub(crate) mod endpoints;
mod events;
pub(super) mod scope;
mod shortcut;
#[cfg(test)]
mod tests;

pub(super) use shortcut::deletes_focused_track;

#[cfg(all(test, feature = "masonry"))]
pub(in crate::gui) use self::compile::compile_studio;
#[cfg(feature = "masonry")]
pub(crate) use self::compile::{entry, resolver};
pub(crate) use self::{
    compile::{StudioUi, view},
    events::translate,
};
