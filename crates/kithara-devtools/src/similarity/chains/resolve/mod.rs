//! Source-only call resolution by declaration identity and typed receiver operations.

mod consts;
mod paths;
mod resolver;
mod traits;
mod types;

use resolver::{Found, TypeDef, module_key};
pub(super) use resolver::{Resolver, Targets, Variant, dedup};
