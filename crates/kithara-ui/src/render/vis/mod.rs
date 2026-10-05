//! Visualiser reads, uniform packing, and toolkit-specific GPU adapters.

#[cfg(all(test, any(feature = "gpu", feature = "masonry")))]
mod fixture;
#[cfg(any(feature = "iced", feature = "masonry"))]
mod frame;
#[cfg(feature = "iced")]
mod iced;
#[cfg(feature = "masonry")]
mod masonry;
#[cfg(any(feature = "iced", feature = "masonry"))]
mod uniform;

#[cfg(any(feature = "iced", feature = "masonry"))]
pub(crate) use frame::VisFrame;
#[cfg(feature = "iced")]
pub(crate) use iced::view;
#[cfg(feature = "masonry")]
pub use masonry::{VisDeclaration, VisPass};
#[cfg(any(feature = "iced", feature = "masonry"))]
pub(crate) use uniform::{Uniforms, consts::SHADER};
