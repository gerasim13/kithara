//! Serializable modular UI model for kithara.

#[cfg(feature = "render")]
pub(crate) mod atoms;
#[cfg(any(feature = "render", feature = "vello-backend"))]
pub mod backends;
pub mod builtin;
pub mod compile;
#[cfg(any(feature = "render", feature = "vello-backend"))]
pub mod draw;
pub mod error;
pub mod expand;
pub mod ids;
pub mod registry;
#[cfg(feature = "render")]
pub mod render;
pub mod size;
pub mod source;
#[cfg(any(feature = "render", feature = "vello-backend"))]
pub mod text;
#[cfg(feature = "render")]
pub mod widgets;

pub use doc::{envelope, layout, module, param, skin};

mod doc;
mod resolve;
mod validate;
