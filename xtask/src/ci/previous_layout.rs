//! What `production/main` tells a job, names in its workflows, and keeps on a
//! host. A host deployed from this branch serves main's workflows until it
//! merges.

use std::{collections::BTreeMap, path::PathBuf};

#[derive(serde::Deserialize)]
pub(crate) struct PreviousLayout {
    pub(crate) repository: Repository,
    pub(crate) linux: Linux,
    pub(crate) mac: Mac,
}

#[derive(serde::Deserialize)]
pub(crate) struct Repository {
    pub(crate) rustc_wrapper: PathBuf,
}

#[derive(serde::Deserialize)]
pub(crate) struct Linux {
    pub(crate) told: BTreeMap<String, PathBuf>,
    pub(crate) named: Named,
}

#[derive(serde::Deserialize)]
pub(crate) struct Named {
    pub(crate) cache_root: PathBuf,
    pub(crate) paths: Vec<PathBuf>,
}

#[derive(serde::Deserialize)]
pub(crate) struct Mac {
    pub(crate) cache_namespaces: Vec<String>,
}

pub(crate) fn previous_layout() -> PreviousLayout {
    toml::from_str(include_str!("previous_layout.toml")).expect("the previous layout parses")
}
