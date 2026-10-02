//! Owned snapshots of retained settings, with builders and accessors.

pub use bon;
/// `#[derive(Config)]` generates the builder, accessors, `Default`, `Debug`,
/// the owned snapshot and explicitly selected runtime updates of a struct,
/// every facet declared through `#[config(...)]`. `construction` classifies a
/// consumed builder input without generating a retained snapshot.
///
/// ```compile_fail
/// #[derive(kithara_config::Config)]
/// struct Unclassified { value: u32 }
/// ```
pub use kithara_derive::Config;
pub use kithara_derive::Patch;

mod config;
pub use config::{Config, ConfigOwner, UpdatableConfig};

#[doc(hidden)]
pub mod __private;
