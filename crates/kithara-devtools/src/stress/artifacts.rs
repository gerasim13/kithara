//! Index preserved artifact roots without copying or overwriting evidence.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};

use super::selection::Unit;

pub(super) struct Evidence {
    pub(super) issues: Vec<String>,
    directories: BTreeMap<PathBuf, Vec<PathBuf>>,
}

impl Evidence {
    pub(super) fn read(roots: &[PathBuf], units: &[Unit<'_>]) -> Result<Self> {
        let expected = units.iter().map(Unit::directory).collect::<BTreeSet<_>>();
        let mut evidence = Self {
            directories: BTreeMap::new(),
            issues: Vec::new(),
        };
        let mut seen = BTreeSet::new();
        for root in roots {
            if !root.is_dir() {
                evidence
                    .issues
                    .push(format!("raw evidence root is missing: {}", root.display()));
                continue;
            }
            let canonical = fs::canonicalize(root)
                .with_context(|| format!("resolve raw evidence root {}", root.display()))?;
            if !seen.insert(canonical.clone()) {
                evidence.issues.push(format!(
                    "raw evidence root is duplicated: {}",
                    root.display()
                ));
                continue;
            }
            evidence.scan(&canonical, Path::new(""), &expected)?;
        }
        for directory in expected {
            match evidence.directories.get(&directory).map(Vec::len) {
                Some(1) => {}
                Some(_) => evidence.issues.push(format!(
                    "unit evidence is duplicated: {}",
                    directory.display()
                )),
                None => evidence
                    .issues
                    .push(format!("unit evidence is missing: {}", directory.display())),
            }
        }
        Ok(evidence)
    }

    pub(super) fn directory(&self, unit: &Unit<'_>) -> Option<&Path> {
        let candidates = self.directories.get(&unit.directory())?;
        match candidates.as_slice() {
            [directory] => Some(directory),
            _ => None,
        }
    }

    fn scan(&mut self, root: &Path, relative: &Path, expected: &BTreeSet<PathBuf>) -> Result<()> {
        let directory = root.join(relative);
        for entry in fs::read_dir(&directory)
            .with_context(|| format!("read raw evidence directory {}", directory.display()))?
        {
            let entry = entry.context("read raw evidence entry")?;
            let kind = entry.file_type().context("inspect raw evidence entry")?;
            let path = relative.join(entry.file_name());
            if kind.is_symlink() {
                self.issues.push(format!(
                    "raw evidence entry must not be a symlink: {}",
                    entry.path().display()
                ));
            } else if kind.is_dir() {
                if expected.contains(&path) {
                    self.directories.entry(path).or_default().push(entry.path());
                } else if expected
                    .iter()
                    .any(|unit| unit.parent() == Some(path.as_path()))
                {
                    self.scan(root, &path, expected)?;
                } else {
                    self.issues.push(format!(
                        "unexpected unit directory: {}",
                        entry.path().display()
                    ));
                }
            }
        }
        Ok(())
    }
}
