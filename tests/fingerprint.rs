use std::{collections::hash_map::DefaultHasher, fs, hash::Hash, path::Path};

/// Hash every Rust source below `dir` and register it as a build input.
pub(crate) fn hash_rs_tree(dir: &Path, hasher: &mut DefaultHasher) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<_> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            hash_rs_tree(&path, hasher);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            if let Ok(bytes) = fs::read(&path) {
                path.to_string_lossy().hash(hasher);
                bytes.hash(hasher);
            }
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::hash::Hasher;

    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test]
    fn rust_tree_fingerprint_changes_with_nested_module_content() {
        let root = tempfile::tempdir().expect("create fingerprint fixture root");
        let module = root.path().join("sync_fixture");
        fs::create_dir(&module).expect("create nested fixture module");
        let source = module.join("analysis_cache.rs");
        fs::write(&source, "const VERSION: u8 = 1;\n").expect("write first fixture source");

        let mut first = DefaultHasher::new();
        hash_rs_tree(root.path(), &mut first);
        let first = first.finish();

        fs::write(&source, "const VERSION: u8 = 2;\n").expect("write changed fixture source");
        let mut second = DefaultHasher::new();
        hash_rs_tree(root.path(), &mut second);

        assert_ne!(first, second.finish());
    }
}
