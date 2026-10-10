use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use anyhow::{Context, Result};
use cargo_metadata::{DependencyKind, Metadata, TargetKind};

pub(super) fn workspace_edges(metadata: &Metadata) -> Result<BTreeMap<String, BTreeSet<String>>> {
    let resolve = metadata
        .resolve
        .as_ref()
        .context("cargo metadata has no dependency resolve")?;
    let members = metadata
        .workspace_packages()
        .into_iter()
        .map(|package| {
            let directory = package
                .manifest_path
                .parent()
                .context("workspace package has no directory")?;
            let relative = directory.strip_prefix(&metadata.workspace_root)?;
            Ok((package.id.clone(), relative.as_str().replace('\\', "/")))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let direct = resolve
        .nodes
        .iter()
        .filter(|node| members.contains_key(&node.id))
        .map(|node| {
            let dependencies = node
                .deps
                .iter()
                .filter(|dependency| {
                    members.contains_key(&dependency.pkg)
                        && dependency
                            .dep_kinds
                            .iter()
                            .any(|kind| kind.kind == DependencyKind::Normal)
                })
                .map(|dependency| dependency.pkg.clone())
                .collect::<BTreeSet<_>>();
            (node.id.clone(), dependencies)
        })
        .collect::<BTreeMap<_, _>>();
    let mut edges = BTreeMap::new();
    for (owner, directory) in &members {
        let mut pending = vec![owner.clone()];
        let mut visited = BTreeSet::new();
        while let Some(package) = pending.pop() {
            if visited.insert(package.clone()) {
                pending.extend(direct.get(&package).into_iter().flatten().cloned());
            }
        }
        visited.remove(owner);
        edges.insert(
            directory.clone(),
            visited
                .iter()
                .filter_map(|package| members.get(package).cloned())
                .collect(),
        );
    }
    Ok(edges)
}

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
        assert!(workspace_edges(&metadata).is_err());
    }

    #[test]
    fn workspace_edges_follow_only_normal_workspace_dependencies() {
        let temp = tempfile::tempdir().expect("workspace directory");
        fs::write(
            temp.path().join("Cargo.toml"),
            "[workspace]\nmembers = ['owner', 'middle', 'leaf', 'dev', 'build']\nexclude = ['foreign']\nresolver = '3'\n",
        )
        .expect("workspace manifest");
        for (name, dependencies) in [
            (
                "owner",
                "[dependencies]\nrenamed = { package = 'middle', path = '../middle' }\nforeign = { path = '../foreign' }\n[dev-dependencies]\ndev = { path = '../dev' }\n[build-dependencies]\nbuild = { path = '../build' }\n",
            ),
            ("middle", "[dependencies]\nleaf = { path = '../leaf' }\n"),
            ("leaf", ""),
            ("dev", ""),
            ("build", ""),
            ("foreign", "[dependencies]\ndev = { path = '../dev' }\n"),
        ] {
            let directory = temp.path().join(name);
            fs::create_dir(&directory).expect("package directory");
            fs::write(
                directory.join("Cargo.toml"),
                format!(
                    "[package]\nname = '{name}'\nversion = '0.0.0'\n[lib]\npath = 'lib.rs'\n{dependencies}",
                ),
            )
            .expect("package manifest");
            fs::write(directory.join("lib.rs"), "").expect("library source");
        }
        let metadata = MetadataCommand::new()
            .manifest_path(temp.path().join("Cargo.toml"))
            .exec()
            .expect("workspace metadata");
        let edges = workspace_edges(&metadata).expect("workspace dependencies");
        assert_eq!(
            edges["owner"],
            BTreeSet::from(["middle".into(), "leaf".into()])
        );
        assert_eq!(edges["middle"], BTreeSet::from(["leaf".into()]));
        for name in ["leaf", "dev", "build"] {
            assert!(edges[name].is_empty());
        }
        assert!(!edges.contains_key("foreign"));
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
