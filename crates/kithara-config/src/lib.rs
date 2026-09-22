//! Owned snapshots of retained settings, with builders and accessors.

pub use kithara_derive::Patch;
/// Composes builders, accessors, snapshots and explicitly selected runtime updates.
///
/// ```compile_fail
/// #[kithara_config::config]
/// struct Unclassified { value: u32 }
/// ```
pub use kithara_derive::config;

mod config;
pub use config::Config;

#[doc(hidden)]
pub mod __private;
