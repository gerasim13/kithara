//! Owned snapshots of retained settings, with builders and accessors.

/// Composes builders and accessors while requiring explicit field roles.
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
