use std::{
    collections::HashSet,
    fs,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result};

/// Every file some `mod` declaration under `src` names, whatever gates it.
///
/// `cargo modules orphans` answers from one resolved configuration and pairs a
/// file with its parent by directory convention, so a module behind a `cfg`
/// this build does not set, or one reached through `#[path]` from a sibling
/// file, reads as unreferenced to it. Both are declared in the source, and this
/// walk reads the declaration instead of the resolution.
pub(super) fn declared_files(src: &Path) -> Result<HashSet<PathBuf>> {
    let mut declared = HashSet::new();
    walk(src, &mut declared)?;
    Ok(declared)
}

fn walk(dir: &Path, declared: &mut HashSet<PathBuf>) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let path = entry
            .with_context(|| format!("walk {}", dir.display()))?
            .path();
        if path.is_dir() {
            walk(&path, declared)?;
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let text =
                fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
            collect(&path, &text, declared);
        }
    }
    Ok(())
}

fn collect(file: &Path, text: &str, declared: &mut HashSet<PathBuf>) {
    let Some(dir) = file.parent() else { return };
    for line in text.lines() {
        let line = line.trim();
        if let Some(target) = path_attribute(line) {
            // A `#[path]` resolves against the directory of the file carrying
            // it, not against the directory the module would own.
            declared.insert(normalize(&dir.join(target)));
        } else if let Some(name) = module_declaration(line) {
            // A crate or module root owns its own directory; any other file
            // owns the directory named after it.
            let base = if owns_its_directory(file) {
                dir.to_owned()
            } else {
                dir.join(file.file_stem().unwrap_or_default())
            };
            declared.insert(base.join(format!("{name}.rs")));
            declared.insert(base.join(name).join("mod.rs"));
        }
    }
}

/// Reads the target of a `#[path]` or `#[cfg_attr(.., path = "..")]`. Every
/// such attribute in this workspace is written on one line, so the value is
/// read where the attribute starts.
fn path_attribute(line: &str) -> Option<&str> {
    if !line.starts_with("#[") {
        return None;
    }
    let mut rest = line;
    while let Some(at) = rest.find("path") {
        rest = rest.get(at + "path".len()..)?;
        let value = rest.trim_start().strip_prefix('=').map(str::trim_start);
        if let Some(quoted) = value.and_then(|value| value.strip_prefix('"')) {
            return quoted.split_once('"').map(|(target, _)| target);
        }
    }
    None
}

fn module_declaration(line: &str) -> Option<&str> {
    let name = strip_visibility(line.strip_suffix(';')?)
        .strip_prefix("mod ")?
        .trim();
    let named = !name.is_empty() && name.chars().all(|ch| ch.is_alphanumeric() || ch == '_');
    named.then_some(name)
}

fn strip_visibility(line: &str) -> &str {
    let line = line.trim_start();
    let Some(rest) = line.strip_prefix("pub") else {
        return line;
    };
    if !rest.starts_with(['(', ' ', '\t']) {
        return line;
    }
    let rest = rest.trim_start();
    // `pub(crate)`, `pub(super)`, `pub(in path::to)`.
    rest.strip_prefix('(')
        .and_then(|scope| scope.split_once(')'))
        .map_or(rest, |(_, after)| after.trim_start())
}

fn owns_its_directory(file: &Path) -> bool {
    file.file_name()
        .is_some_and(|name| name == "mod.rs" || name == "lib.rs" || name == "main.rs")
}

/// Resolves `..` lexically, so a `#[path]` that climbs out of its directory
/// compares equal to the path the tool reports.
pub(super) fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    fn declared(files: &[(&str, &str)]) -> HashSet<PathBuf> {
        let temp = tempdir().expect("tempdir");
        for (name, contents) in files {
            let path = temp.path().join(name);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create source directory");
            }
            fs::write(&path, contents).expect("write source");
        }
        let found = declared_files(temp.path()).expect("declared files");
        found
            .into_iter()
            .map(|path| {
                path.strip_prefix(temp.path())
                    .unwrap_or(&path)
                    .to_path_buf()
            })
            .collect()
    }

    fn names(files: &[(&str, &str)]) -> HashSet<String> {
        declared(files)
            .into_iter()
            .map(|path| path.display().to_string())
            .collect()
    }

    #[test]
    fn a_plain_declaration_in_a_module_root_names_its_sibling() {
        let found = names(&[("mod.rs", "mod detector;\n")]);

        assert!(found.contains("detector.rs"));
    }

    #[test]
    fn a_plain_declaration_in_a_leaf_file_names_its_own_directory() {
        let found = names(&[("scan.rs", "mod detail;\n")]);

        assert!(found.contains("scan/detail.rs"));
    }

    /// The gate exists to find files nothing loads, so a `cfg` that is off in
    /// this build must not turn a declared module into one.
    #[test]
    fn a_cfg_gated_declaration_still_counts_as_declared() {
        let found = names(&[("mod.rs", "#[cfg(target_os = \"android\")]\nmod android;\n")]);

        assert!(found.contains("android/mod.rs"));
    }

    #[test]
    fn a_path_attribute_resolves_against_the_declaring_file() {
        let found = names(&[(
            "mp4/scan.rs",
            "#[cfg(test)]\n#[path = \"tests.rs\"]\nmod tests;\n",
        )]);

        assert!(found.contains("mp4/tests.rs"));
    }

    #[test]
    fn a_path_attribute_climbs_out_of_its_directory() {
        let found = names(&[(
            "backend/system.rs",
            "#[path = \"../system/sync/mod.rs\"]\nmod sync;\n",
        )]);

        assert!(found.contains("system/sync/mod.rs"));
    }

    #[test]
    fn a_cfg_attr_path_counts_as_declared() {
        let found = names(&[(
            "broadcast/mod.rs",
            "#[cfg_attr(feature = \"broadcast\", path = \"live.rs\")]\nmod backend;\n",
        )]);

        assert!(found.contains("broadcast/live.rs"));
    }

    /// One module, two files: whichever feature is off, the other file is
    /// still loaded by some build.
    #[test]
    fn both_arms_of_a_swapped_module_count_as_declared() {
        let found = names(&[(
            "timestretch/mod.rs",
            "#[cfg(feature = \"bungee\")]\n#[path = \"backend.rs\"]\nmod backend;\n\
             #[cfg(not(feature = \"bungee\"))]\n#[path = \"controls.rs\"]\nmod backend;\n",
        )]);

        assert!(
            found.contains("timestretch/backend.rs") && found.contains("timestretch/controls.rs")
        );
    }

    #[test]
    fn a_file_no_declaration_names_stays_undeclared() {
        let found = names(&[("mod.rs", "mod detector;\n"), ("stray.rs", "")]);

        assert!(!found.contains("stray.rs"));
    }

    #[test]
    fn a_visibility_qualifier_does_not_hide_the_declaration() {
        let found = names(&[("mod.rs", "pub(crate) mod system;\n")]);

        assert!(found.contains("system.rs"));
    }

    /// `path = "..."` in ordinary code is a string, not a declaration.
    #[test]
    fn a_path_assignment_outside_an_attribute_is_not_a_declaration() {
        let found = names(&[("mod.rs", "fn probe() {\n    let path = \"stray.rs\";\n}\n")]);

        assert!(!found.contains("stray.rs"));
    }
}
