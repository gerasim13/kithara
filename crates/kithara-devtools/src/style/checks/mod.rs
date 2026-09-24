//! Registry of code-style checks.
//!
//! Style checks operate on intra-file organisation (constant locality, field
//! and item ordering, init-expression form). They are independent from `arch`
//! topology checks: a single file can satisfy `arch` perfectly and still trip
//! `style` rules, and vice versa.

pub(crate) mod comment_hygiene;
pub(crate) mod const_locality;
pub(crate) mod dead_doc_refs;
pub(crate) mod doc_size;
pub(crate) mod doc_staleness;
pub(crate) mod non_english_text;
pub(crate) mod qualified_path_depth;
pub(crate) mod readme_shape;
mod registry;
pub(crate) mod split_module;
pub(crate) mod struct_field_order;
pub(crate) mod struct_init_order;
pub(crate) mod thin_module_dir;
pub(crate) mod trait_item_order;

pub(crate) use registry::{Check, Context, registry};
