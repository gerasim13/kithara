mod backend;
#[cfg(test)]
mod tests;

#[cfg(feature = "masonry")]
pub(super) use self::backend::has_system_text;
pub use self::backend::{VelloBackend, paint_color};
