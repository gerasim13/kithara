//! Photographing a page with no window, and comparing two sets of photographs.
//!
//! A host draws the same documents through two engines, and the only way to
//! see that they agree is to photograph both and count where the pixels
//! disagree. That harness belongs beside the engines rather than in whichever
//! application happens to run it, and it is behind the `capture` feature so a
//! shipped build carries none of it.
//!
//! The feature gate stands here rather than on the declaration in `lib.rs`,
//! the way `app` carries its own: one gate on the module is one gate, and the
//! crate root already carries as many as it can hold.
#![cfg(feature = "capture")]

pub mod diff;
mod film;
mod geometry;
mod photo;
#[cfg(feature = "masonry")]
mod scene;
mod set;
mod stage;

pub use film::{Film, page_file};
pub use geometry::{Geometry, read_geometry, write_geometry, write_png};
pub use photo::Photographer;
#[cfg(feature = "masonry")]
pub use scene::Offscreen;
pub use set::shoot_set;
pub use stage::Stage;
