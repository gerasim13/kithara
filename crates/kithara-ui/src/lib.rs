//! Serializable modular UI model for kithara.

pub mod app;
#[cfg(feature = "render")]
pub(crate) mod atoms;
#[cfg(any(feature = "render", feature = "vello"))]
pub mod backends;
pub mod builtin;
pub mod capture;
pub mod compile;
#[cfg(any(feature = "iced", feature = "masonry"))]
pub(crate) mod engine;
pub mod error;
pub mod expand;
pub mod ids;
#[cfg(any(test, feature = "mock"))]
pub mod mock;
pub(crate) mod mount;
pub mod registry;
#[cfg(feature = "render")]
pub mod render;
pub mod size;
#[cfg(any(feature = "iced", feature = "masonry"))]
pub(crate) mod solve;
pub mod source;
pub mod view;

pub use doc::{envelope, layout, module, package, param, skin, text};
pub use kithara_ui_draw::geom;
pub use kithara_ui_input as interact;
#[cfg(feature = "render")]
pub use {kithara_ui_draw as draw, kithara_ui_lottie as lottie, kithara_ui_shaping as shaping};

mod doc;
mod resolve;
mod room;
mod shader;
mod validate;
