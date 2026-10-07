//! What each check concluded about each file, kept between runs.
//!
//! A ratchet run parses the same source once per check, and almost every file
//! is byte-identical to the previous run. A check whose findings about a file
//! depend on that file alone is handed a view of the workspace holding exactly
//! that file, and what it finds is kept under the file's path beside the
//! digest of the bytes it judged. A later run that finds the same bytes at the
//! same path reads the verdict back instead of parsing and analysing the file
//! again.
//!
//! A verdict is a function of the program that reached it, the configuration
//! that program was given, the workspace, and the file. The file is the digest
//! kept beside each verdict; the rest fold into one salt that a store carries
//! whole, so a store another build of the linter wrote, or one written under
//! another configuration, reads as empty and is replaced rather than served.
//!
//! Each check keeps one store per namespace, so the cache never holds more
//! than one store per check, and two checks running in parallel never write
//! the same file. A store is replaced through a rename: a concurrent run reads
//! the previous store or the whole new one, and when two runs replace it at
//! once the later one's verdicts stay and the earlier one's are reached again
//! next time.
//!
//! What is kept is the verdict, not the syntax tree: `syn::File` holds
//! `proc_macro2` spans, which are neither `Send`, `Sync`, nor serialisable.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::{scan::Scan, violation::Violation, walker::relative_to};

mod consts {
    /// The directory under Cargo's build directory that holds every
    /// namespace's stores.
    pub(super) const DIR: &str = "kithara-verdict-cache";
}

/// One check's verdicts, as one store.
#[derive(Deserialize, Serialize)]
struct Store {
    /// What the verdicts were reached by and under; see `VerdictCache::open`.
    salt: String,
    /// Each judged file by its workspace-relative path.
    files: BTreeMap<String, Verdict>,
}

/// What a check found in one file.
#[derive(Deserialize, Serialize)]
struct Verdict {
    /// The digest of the bytes the check judged.
    digest: String,
    violations: Vec<Violation>,
}

/// A namespace's stores of verdicts.
#[derive(Debug)]
pub struct VerdictCache {
    dir: PathBuf,
    workspace_root: PathBuf,
    salt: String,
}

impl VerdictCache {
    /// Opens `namespace`'s stores under Cargo's build directory `target_dir`.
    ///
    /// `program` is the executable reaching the verdicts, and `config` the
    /// configuration its checks were loaded with, as they see it. With
    /// `workspace_root` they make the salt every store is read against. The
    /// program counts by its size and modification time: Cargo links a
    /// binary anew on every build, so another build of the linter never
    /// reads as this one.
    ///
    /// # Errors
    ///
    /// When the program's metadata cannot be read.
    pub fn open(
        target_dir: &Path,
        namespace: &str,
        program: &Path,
        workspace_root: &Path,
        config: &str,
    ) -> Result<Self> {
        let metadata = fs::metadata(program)
            .with_context(|| format!("read the linter's metadata {}", program.display()))?;
        let linked = metadata
            .modified()
            .with_context(|| format!("read when {} was linked", program.display()))?
            .duration_since(UNIX_EPOCH)
            .context("the linter was linked before 1970")?
            .as_nanos();
        let mut hasher = Sha256::new();
        for part in [
            &metadata.len().to_le_bytes()[..],
            &linked.to_le_bytes(),
            workspace_root.as_os_str().as_encoded_bytes(),
            config.as_bytes(),
        ] {
            hasher.update(part.len().to_le_bytes());
            hasher.update(part);
        }
        Ok(Self {
            dir: target_dir.join(consts::DIR).join(namespace),
            workspace_root: workspace_root.to_path_buf(),
            salt: hex::encode(hasher.finalize()),
        })
    }

    /// The violations `check` finds in `files`.
    ///
    /// A file whose kept verdict names the bytes `scan` read is not judged
    /// again. Any other file is handed to `judge` as a view of the workspace
    /// holding that file alone, and its verdict is kept. A file that cannot
    /// be read is judged the same way and nothing about it is kept: the check
    /// decides what an unreadable file means, as it does over a whole scope.
    /// The store keeps the verdicts for files outside `files`, so a run over
    /// part of the workspace leaves the rest for the next full run.
    ///
    /// # Errors
    ///
    /// When `judge` fails, or the store cannot be replaced.
    pub fn verdicts<F>(
        &self,
        check: &'static str,
        scan: &Scan,
        files: &[PathBuf],
        judge: F,
    ) -> Result<Vec<Violation>>
    where
        F: Fn(&Scan) -> Result<Vec<Violation>>,
    {
        let path = self.dir.join(format!("{check}.json"));
        let mut store = read_store(&path)
            .filter(|store| store.salt == self.salt)
            .unwrap_or_else(|| Store {
                salt: self.salt.clone(),
                files: BTreeMap::new(),
            });
        let mut judged = false;
        let mut violations = Vec::new();
        for file in files {
            let Some(digest) = scan.digest(file).map(hex::encode) else {
                violations.extend(judge(&scan.for_file(file))?);
                continue;
            };
            let rel = relative_to(&self.workspace_root, file)
                .to_string_lossy()
                .replace('\\', "/");
            if let Some(kept) = store.files.get(&rel).filter(|kept| kept.digest == digest) {
                violations.extend(kept.violations.iter().map(|violation| Violation {
                    check,
                    ..violation.clone()
                }));
                continue;
            }
            let found = judge(&scan.for_file(file))?;
            violations.extend(found.iter().cloned());
            store.files.insert(
                rel,
                Verdict {
                    digest,
                    violations: found,
                },
            );
            judged = true;
        }
        if judged {
            write_store(&path, &store)?;
        }
        Ok(violations)
    }
}

/// A store that is missing or cannot be read holds no verdicts: the run
/// judges every file and writes a current store in its place.
fn read_store(path: &Path) -> Option<Store> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

/// Writes through a temporary name in the same directory, so a concurrent
/// reader sees either the previous store or the whole new one.
fn write_store(path: &Path, store: &Store) -> Result<()> {
    let parent = path.parent().context("a verdict store has no directory")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("create the verdict cache {}", parent.display()))?;
    let staged = path.with_extension(format!("{}.tmp", std::process::id()));
    fs::write(&staged, serde_json::to_vec(store)?)
        .with_context(|| format!("write the verdict store {}", staged.display()))?;
    fs::rename(&staged, path)
        .with_context(|| format!("publish the verdict store {}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::common::scope::Scope;

    /// A workspace of `files`, a linter binary standing in for the program,
    /// and the build directory the stores live in.
    struct Fixture {
        dir: tempfile::TempDir,
    }

    impl Fixture {
        fn new(files: &[(&str, &str)]) -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            for (name, text) in files {
                fs::write(dir.path().join(name), text).expect("write a source");
            }
            fs::write(dir.path().join("linter"), "build one").expect("write the program");
            Self { dir }
        }

        fn file(&self, name: &str) -> PathBuf {
            self.dir.path().join(name)
        }

        fn cache(&self, program: &str, config: &str) -> VerdictCache {
            VerdictCache::open(
                &self.dir.path().join("target"),
                "probe",
                &self.file(program),
                self.dir.path(),
                config,
            )
            .expect("open the cache")
        }

        /// The keys a fresh run of a check that reports each file it judges
        /// finds in `names`, and how many files it judged.
        fn run(&self, cache: &VerdictCache, names: &[&str]) -> (Vec<String>, usize) {
            let scan = Scan::new(self.dir.path());
            let judged = AtomicUsize::new(0);
            let files: Vec<PathBuf> = names.iter().map(|name| self.file(name)).collect();
            let found = cache
                .verdicts("probe", &scan, &files, |view| {
                    judged.fetch_add(1, Ordering::Relaxed);
                    let seen = view.rs_files(&Scope::default())?;
                    Ok(seen
                        .iter()
                        .map(|path| {
                            let length = view.source(path).map_or(0, |source| source.len());
                            Violation::warn("probe", format!("{}:{length}", path.display()), "seen")
                                .with_explanation(format!("judged {}", path.display()).into())
                        })
                        .collect())
                })
                .expect("verdicts");
            assert!(found.iter().all(|violation| violation.check == "probe"));
            (
                found.into_iter().map(|violation| violation.key).collect(),
                judged.into_inner(),
            )
        }
    }

    #[test]
    fn a_file_is_judged_once_and_its_verdict_read_back() {
        let fixture = Fixture::new(&[("lib.rs", "fn main() {}")]);

        let cold = fixture.run(&fixture.cache("linter", "config"), &["lib.rs"]);
        let warm = fixture.run(&fixture.cache("linter", "config"), &["lib.rs"]);

        assert_eq!(cold.1, 1);
        assert_eq!(warm, (cold.0, 0));
    }

    #[test]
    fn a_changed_file_is_judged_again() {
        let fixture = Fixture::new(&[("lib.rs", "fn main() {}")]);
        fixture.run(&fixture.cache("linter", "config"), &["lib.rs"]);

        fs::write(fixture.file("lib.rs"), "fn main() { body(); }").expect("rewrite");
        let (keys, judged) = fixture.run(&fixture.cache("linter", "config"), &["lib.rs"]);

        assert_eq!(judged, 1);
        assert_eq!(keys, [format!("{}:21", fixture.file("lib.rs").display())]);
    }

    #[test]
    fn files_with_the_same_bytes_keep_their_own_findings() {
        let fixture = Fixture::new(&[("one.rs", "fn same() {}"), ("two.rs", "fn same() {}")]);

        let cold = fixture.run(&fixture.cache("linter", "config"), &["one.rs", "two.rs"]);
        let warm = fixture.run(&fixture.cache("linter", "config"), &["one.rs", "two.rs"]);

        assert_eq!(
            cold.0,
            [
                format!("{}:12", fixture.file("one.rs").display()),
                format!("{}:12", fixture.file("two.rs").display()),
            ]
        );
        assert_eq!(warm, (cold.0, 0));
    }

    #[test]
    fn another_build_of_the_linter_judges_every_file_again() {
        let fixture = Fixture::new(&[("lib.rs", "fn main() {}")]);
        fixture.run(&fixture.cache("linter", "config"), &["lib.rs"]);
        fs::write(fixture.file("rebuilt"), "build two, linked later").expect("write");

        let (_, judged) = fixture.run(&fixture.cache("rebuilt", "config"), &["lib.rs"]);

        assert_eq!(judged, 1);
    }

    #[test]
    fn another_configuration_judges_every_file_again() {
        let fixture = Fixture::new(&[("lib.rs", "fn main() {}")]);
        fixture.run(&fixture.cache("linter", "config"), &["lib.rs"]);

        let (_, judged) = fixture.run(&fixture.cache("linter", "raised"), &["lib.rs"]);

        assert_eq!(judged, 1);
    }

    #[test]
    fn a_store_replaced_under_another_salt_does_not_grow() {
        let fixture = Fixture::new(&[("lib.rs", "fn main() {}")]);
        fixture.run(&fixture.cache("linter", "config"), &["lib.rs"]);
        fixture.run(&fixture.cache("linter", "raised"), &["lib.rs"]);

        let stores: Vec<_> = fs::read_dir(
            fixture
                .dir
                .path()
                .join("target")
                .join(consts::DIR)
                .join("probe"),
        )
        .expect("the namespace's stores")
        .map(|entry| entry.expect("entry").file_name())
        .collect();

        assert_eq!(stores, ["probe.json"]);
    }

    #[test]
    fn a_partial_run_keeps_the_verdicts_it_did_not_reach() {
        let fixture = Fixture::new(&[("one.rs", "fn one() {}"), ("two.rs", "fn two() {}")]);
        fixture.run(&fixture.cache("linter", "config"), &["one.rs", "two.rs"]);
        fs::write(fixture.file("one.rs"), "fn one() { changed(); }").expect("rewrite");
        fixture.run(&fixture.cache("linter", "config"), &["one.rs"]);

        let (_, judged) = fixture.run(&fixture.cache("linter", "config"), &["one.rs", "two.rs"]);

        assert_eq!(judged, 0);
    }

    #[test]
    fn an_unreadable_file_is_judged_every_time() {
        let fixture = Fixture::new(&[]);
        fixture.run(&fixture.cache("linter", "config"), &["missing.rs"]);

        let (_, judged) = fixture.run(&fixture.cache("linter", "config"), &["missing.rs"]);

        assert_eq!(judged, 1);
    }
}
