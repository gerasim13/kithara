//! Owned snapshots of retained settings, with builders and accessors.

pub use bon;
/// `#[derive(Config)]` generates the builder, accessors, `Default`, `Debug`,
/// the owned snapshot and explicitly selected runtime updates of a struct,
/// every facet declared through `#[config(...)]`. `construction` treats unmarked
/// fields as consumed builder inputs and generates no retained snapshot. On a struct
/// `fields(...)` supplies field defaults using the same grammar as a field
/// declaration, for example `fields(value, get(copy), builder(default))`.
/// `get(ref)` borrows the retained field; `get(skip)` disables an inherited
/// getter. Explicit field facets override defaults; groups replace whole groups.
///
/// ```compile_fail
/// #[derive(kithara_config::Config)]
/// struct Unclassified { value: u32 }
/// ```
///
/// An owner without a check takes updates in place, so it nests only
/// configurations that cannot refuse:
///
/// ```compile_fail
/// #[derive(Clone, kithara_config::Config)]
/// #[config(update, patch(validate = Self::checked, error = std::io::Error))]
/// struct Inner {
///     #[config(value, update)]
///     level: u32,
/// }
///
/// impl Inner {
///     fn checked(self) -> Result<Self, std::io::Error> {
///         Ok(self)
///     }
/// }
///
/// #[derive(kithara_config::Config)]
/// #[config(update)]
/// struct Outer {
///     #[config(nested, update)]
///     inner: Inner,
/// }
/// ```
pub use kithara_derive::Config;
pub use kithara_derive::{ConfigOwner, Patch};

mod config;
pub use config::{Config, ConfigOwner, ConfigOwnerMut, UpdatableConfig};

#[doc(hidden)]
pub mod __private;
