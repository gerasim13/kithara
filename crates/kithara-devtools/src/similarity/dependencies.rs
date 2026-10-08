use std::{collections::BTreeSet, path::PathBuf};

use anyhow::{Context, Result};
use cargo_metadata::{Metadata, TargetKind};

pub(super) fn roots(metadata: &Metadata) -> Result<BTreeSet<(String, PathBuf)>> {
    let resolve = metadata
        .resolve
        .as_ref()
        .context("cargo metadata has no dependency resolve")?;
    Ok(resolve
        .nodes
        .iter()
        .filter(|node| metadata.workspace_members.contains(&node.id))
        .flat_map(|node| &node.deps)
        .filter_map(|dependency| {
            let package = metadata
                .packages
                .iter()
                .find(|package| package.id == dependency.pkg)?;
            let target = package
                .targets
                .iter()
                .find(|target| target.kind.contains(&TargetKind::Lib))?;
            Some((
                dependency.name.clone(),
                target.src_path.as_std_path().to_path_buf(),
            ))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use cargo_metadata::MetadataCommand;

    use super::*;

    #[test]
    fn dependency_roots_reject_metadata_without_resolve() {
        let temp = workspace("synthetic-package = { path = '../foreign' }");
        let metadata = MetadataCommand::new()
            .manifest_path(temp.path().join("Cargo.toml"))
            .no_deps()
            .exec()
            .expect("workspace metadata");
        assert!(roots(&metadata).is_err());
    }

    #[test]
    fn dependency_roots_follow_metadata_import_names_and_lib_paths() {
        for (entry, import) in [
            (
                "renamed_api = { package = 'synthetic-package', path = '../foreign' }",
                "renamed_api",
            ),
            (
                "synthetic-package = { path = '../foreign' }",
                "borrowed_api",
            ),
        ] {
            let temp = workspace(entry);
            let metadata = MetadataCommand::new()
                .manifest_path(temp.path().join("Cargo.toml"))
                .exec()
                .expect("workspace metadata");
            assert_eq!(
                roots(&metadata).expect("dependency roots"),
                [(import.to_owned(), temp.path().join("foreign/api.rs"))]
                    .into_iter()
                    .collect()
            );
        }
    }

    fn workspace(entry: &str) -> tempfile::TempDir {
        let temp = tempfile::tempdir().expect("workspace directory");
        for directory in ["owner", "foreign", "transitive"] {
            fs::create_dir(temp.path().join(directory)).expect("package directory");
        }
        fs::write(
            temp.path().join("Cargo.toml"),
            "[workspace]\nmembers = ['owner']\nexclude = ['foreign', 'transitive']\nresolver = '3'\n",
        )
        .expect("workspace manifest");
        fs::write(
            temp.path().join("owner/Cargo.toml"),
            format!("[package]\nname = 'synthetic-owner'\nversion = '0.0.0'\n[lib]\npath = 'lib.rs'\n[dependencies]\n{entry}\n"),
        )
        .expect("owner manifest");
        fs::write(
            temp.path().join("foreign/Cargo.toml"),
            "[package]\nname = 'synthetic-package'\nversion = '0.0.0'\n[lib]\nname = 'borrowed_api'\npath = 'api.rs'\n[dependencies]\nsynthetic-transitive = { path = '../transitive' }\n",
        )
        .expect("dependency manifest");
        fs::write(
            temp.path().join("transitive/Cargo.toml"),
            "[package]\nname = 'synthetic-transitive'\nversion = '0.0.0'\n[lib]\npath = 'lib.rs'\n",
        )
        .expect("transitive manifest");
        for file in ["owner/lib.rs", "foreign/api.rs", "transitive/lib.rs"] {
            fs::write(temp.path().join(file), "").expect("library source");
        }
        temp
    }
}
