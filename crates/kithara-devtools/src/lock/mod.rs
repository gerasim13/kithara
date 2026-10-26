mod file;
#[cfg(unix)]
pub(crate) mod filesystem;

#[cfg(unix)]
pub use file::open_at;
pub use file::{FileLock, Wait, open};
