//! Layer the application configuration document and bake the references it
//! names, shared by the application's build script and its startup.

mod bake;
mod merge;

pub use bake::{Bake, bake};
pub use merge::merge;
