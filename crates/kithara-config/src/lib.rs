//! Owned snapshots of retained settings, with builders and accessors.

pub use bon;
/// `#[derive(Config)]` generates the builder, accessors, `Default`, `Debug`,
/// the owned snapshot and explicitly selected runtime updates of a struct,
/// every facet declared through `#[config(...)]`. `construction` treats unmarked
/// fields as consumed builder inputs and generates no retained snapshot. On a struct
/// whose fields are values by default, `#[config(fields(value))]` supplies the
/// role for unannotated fields; an explicit field role overrides it.
///
/// ```compile_fail
/// #[derive(kithara_config::Config)]
/// struct Unclassified { value: u32 }
/// ```
pub use kithara_derive::Config;
pub use kithara_derive::{ConfigOwner, Patch};

mod config;
pub use config::{Config, ConfigOwner, ConfigOwnerMut, UpdatableConfig};

#[doc(hidden)]
pub mod __private;
