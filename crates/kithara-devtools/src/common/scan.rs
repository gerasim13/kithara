//! One walk of the workspace and one read of each file, shared by every check
//! in a ratchet run.
//!
//! Each check used to walk the tree itself, so a 46-check `idioms` run walked
//! it 46 times and read the same 18 MB of source 42 times. The driver owns
//! that work now: the first asker pays and the rest read the answer.
//!
//! The parsed AST is deliberately not shared. `syn::File` holds `proc_macro2`
//! spans, which are neither `Send` nor `Sync`, so it belongs to the one check
//! that asked for it. Sharing the source text means that parse no longer waits
//! on disk. Entries fill on demand, because `--check` runs only a subset. An
//! unreadable file is remembered as absent, as each check already treated it.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use anyhow::{Context as _, Result};

use super::{
    scope::Scope,
    walker::{workspace_rs_files_scoped, workspace_text_files_scoped},
};

#[derive(Debug)]
pub struct Scan {
    workspace_root: PathBuf,
    rs_files: RwLock<HashMap<Scope, Arc<Vec<PathBuf>>>>,
    sources: RwLock<HashMap<PathBuf, Option<Arc<String>>>>,
    text_files: RwLock<HashMap<Scope, Arc<Vec<PathBuf>>>>,
}

impl Scan {
    #[must_use]
    pub fn new(workspace_root: &Path) -> Self {
        Self {
            workspace_root: workspace_root.to_path_buf(),
            rs_files: RwLock::new(HashMap::new()),
            sources: RwLock::new(HashMap::new()),
            text_files: RwLock::new(HashMap::new()),
        }
    }

    /// Read-check-write under two short locks rather than one long one: the
    /// walk runs unlocked, so a second scope asking at the same time is not
    /// held behind it. A race merely walks twice and keeps the later answer,
    /// which is the same answer.
    fn memoise<K, V, F>(cell: &RwLock<HashMap<K, Arc<V>>>, key: &K, produce: F) -> Result<Arc<V>>
    where
        K: Clone + Eq + std::hash::Hash,
        F: FnOnce() -> Result<V>,
    {
        if let Some(hit) = cell.read().ok().and_then(|map| map.get(key).cloned()) {
            return Ok(hit);
        }
        let value = Arc::new(produce()?);
        if let Ok(mut map) = cell.write() {
            map.insert(key.clone(), Arc::clone(&value));
        }
        Ok(value)
    }

    /// The parsed form of `path`, from the shared source text.
    ///
    /// The AST itself is not cached: `syn::File` holds `proc_macro2` spans and
    /// is neither `Send` nor `Sync`, so it belongs to the one check that asked.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or does not parse as Rust.
    pub fn parse_file(&self, path: &Path) -> Result<syn::File> {
        let source = self
            .source(path)
            .with_context(|| format!("read {}", path.display()))?;
        Ok(syn::parse_file(&source)?)
    }

    /// The `.rs` files under `scope`, walked once per distinct scope.
    ///
    /// # Errors
    ///
    /// Returns an error if the walk fails, the same one the underlying walker
    /// would have returned to the first caller.
    pub fn rs_files(&self, scope: &Scope) -> Result<Arc<Vec<PathBuf>>> {
        Self::memoise(&self.rs_files, scope, || {
            workspace_rs_files_scoped(&self.workspace_root, scope)
        })
    }

    /// The contents of `path`, read once, or `None` if it cannot be read.
    #[must_use]
    pub fn source(&self, path: &Path) -> Option<Arc<String>> {
        if let Some(hit) = self
            .sources
            .read()
            .ok()
            .and_then(|map| map.get(path).cloned())
        {
            return hit;
        }
        let read = std::fs::read_to_string(path).ok().map(Arc::new);
        if let Ok(mut map) = self.sources.write() {
            map.insert(path.to_path_buf(), read.clone());
        }
        read
    }

    /// The tracked text files under `scope`, walked once per distinct scope.
    ///
    /// # Errors
    ///
    /// Returns an error if the walk fails, the same one the underlying walker
    /// would have returned to the first caller.
    pub fn text_files(&self, scope: &Scope) -> Result<Arc<Vec<PathBuf>>> {
        Self::memoise(&self.text_files, scope, || {
            workspace_text_files_scoped(&self.workspace_root, scope)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_source_is_read_once_and_remembered() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("lib.rs");
        std::fs::write(&path, "fn main() {}").expect("write");
        let scan = Scan::new(dir.path());

        let first = scan.source(&path).expect("read");
        std::fs::write(&path, "fn other() {}").expect("rewrite");
        let second = scan.source(&path).expect("cached");

        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(second.as_str(), "fn main() {}");
    }

    #[test]
    fn an_unreadable_path_is_remembered_as_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let scan = Scan::new(dir.path());

        assert!(scan.source(&dir.path().join("missing.rs")).is_none());
        assert!(scan.source(&dir.path().join("missing.rs")).is_none());
    }

    #[test]
    fn two_scopes_get_their_own_file_lists() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("crates/one/src")).expect("mkdir");
        std::fs::create_dir_all(dir.path().join("crates/two/src")).expect("mkdir");
        std::fs::write(dir.path().join("crates/one/src/lib.rs"), "").expect("write");
        std::fs::write(dir.path().join("crates/two/src/lib.rs"), "").expect("write");
        let scan = Scan::new(dir.path());

        let all = scan.rs_files(&Scope::default()).expect("walk");
        let one = scan
            .rs_files(&Scope::new(vec!["one".into()], vec![]))
            .expect("walk");

        assert_eq!(all.len(), 2);
        assert_eq!(one.len(), 1);
        assert!(Arc::ptr_eq(
            &all,
            &scan.rs_files(&Scope::default()).expect("cached")
        ));
    }
}
