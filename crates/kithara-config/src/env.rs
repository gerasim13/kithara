use std::{env, path::PathBuf};

/// Reads a native path for a configuration default; unset or empty means absent.
pub fn path_from_env(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}
