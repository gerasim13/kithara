//! A checksum lane's cargo checksums what rustc read, so a rustc unit is
//! rebuilt exactly when its inputs changed, whoever built it last. A
//! build-script run is still judged by the mtimes of what it named, which a
//! persistent checkout and another branch's build make meaningless. So the
//! claim decides every run itself: a run is kept only while everything it
//! named holds the content it was run against, and is otherwise removed so
//! cargo runs it again. Every artifact left is then aligned to the claim
//! instant, so no dependency reads newer than its dependent. The checkout
//! itself is never written.

use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs::{self, File},
    io,
    path::{Component, Path, PathBuf},
    time::SystemTime,
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::{
    layout::{profiles, subdirectories},
    tracked::{self, Sources},
};
use crate::consts;

/// One build-script run cargo left in the lane directory.
pub(super) struct Unit {
    /// The run directory relative to the lane directory, `/`-separated.
    pub(super) key: String,
    /// The run's `output`, whose removal makes cargo run the script again.
    pub(super) output: PathBuf,
    /// The workspace package the script belongs to; `None` for a dependency.
    pub(super) root: Option<PathBuf>,
    /// What the run told cargo it read.
    pub(super) watches: Watches,
}

impl Unit {
    /// Whether the script belongs to a workspace package.
    pub(super) const fn is_workspace(&self) -> bool {
        self.root.is_some()
    }
}

/// The paths and variables a run named with `rerun-if` directives.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Watches {
    /// Every `rerun-if-changed` path, joined to the package root when
    /// relative. A dependency's relative path names its own sources, which its
    /// version fixes, so it is left out.
    pub(super) paths: Vec<PathBuf>,
    /// Whether the run printed any rerun-if directive at all.
    pub(super) declares: bool,
}

/// What a kept run was run against.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct UnitRecord {
    /// The mtime of the run's `output`; a run another job made since differs.
    output_modified: SystemTime,
    /// The content each watched path inside the checkout named.
    watched: BTreeMap<String, String>,
}

/// Every kept run, by key.
pub(super) type Records = BTreeMap<String, UnitRecord>;

/// Where a watched path lies.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Place {
    /// Inside the lane directory.
    Build,
    /// Inside the checkout, by its `/`-separated relative path.
    Checkout(String),
    /// Anywhere else.
    Outside(PathBuf),
}

/// A file's mtime as the filesystem keeps it.
fn modified(path: &Path) -> Result<SystemTime> {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .with_context(|| format!("reading the mtime of {}", path.display()))
}

/// The checkout a claim judges runs against, and the slot it holds.
pub(super) struct Checkout {
    pub(super) root: PathBuf,
    pub(super) build: PathBuf,
    pub(super) tracked: Sources,
}

impl Checkout {
    pub(super) fn new(root: &Path, build: &Path, tracked: Sources) -> Result<Self> {
        Ok(Self {
            root: fs::canonicalize(root)
                .with_context(|| format!("resolving {}", root.display()))?,
            build: fs::canonicalize(build)
                .with_context(|| format!("resolving {}", build.display()))?,
            tracked,
        })
    }

    /// Where `path` lies. A path that does not exist is placed as written.
    pub(super) fn classify(&self, path: &Path) -> Result<Place> {
        let path = match fs::canonicalize(path) {
            Ok(path) => path,
            Err(error) if error.kind() == io::ErrorKind::NotFound => path.to_path_buf(),
            Err(error) => {
                return Err(error).with_context(|| format!("resolving {}", path.display()));
            }
        };
        if path.starts_with(&self.build) {
            return Ok(Place::Build);
        }
        let Ok(relative) = path.strip_prefix(&self.root) else {
            return Ok(Place::Outside(path));
        };
        let name = relative
            .components()
            .filter_map(|component| match component {
                Component::Normal(part) => Some(part.to_string_lossy()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("/");
        Ok(Place::Checkout(name))
    }
}

/// The rerun-if directives in a run's `output`.
fn parse_output(text: &str, root: Option<&Path>) -> Watches {
    let mut watches = Watches::default();
    for line in text.lines() {
        let Some(directive) = line
            .strip_prefix("cargo::")
            .or_else(|| line.strip_prefix("cargo:"))
        else {
            continue;
        };
        if let Some(path) = directive.strip_prefix("rerun-if-changed=") {
            watches.declares = true;
            let path = Path::new(path);
            match root {
                Some(root) if path.is_relative() => watches.paths.push(root.join(path)),
                None if path.is_relative() => {}
                _ => watches.paths.push(path.to_path_buf()),
            }
        } else if directive.starts_with("rerun-if-env-changed=") {
            watches.declares = true;
        }
    }
    watches
}

/// Every build-script run under the lane's profiles, and what each named.
pub(super) fn discover(dir: &Path, packages: &BTreeMap<String, PathBuf>) -> Result<Vec<Unit>> {
    let mut units = Vec::new();
    for profile in profiles(dir)? {
        for run in subdirectories(&profile.join("build"))? {
            let output = run.join("output");
            let text = match fs::read_to_string(&output) {
                Ok(text) => text,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(error).with_context(|| format!("reading {}", output.display()));
                }
            };
            let root = run
                .file_name()
                .and_then(OsStr::to_str)
                .and_then(|name| name.rsplit_once('-'))
                .and_then(|(package, _)| packages.get(package))
                .cloned();
            let key = run
                .strip_prefix(dir)
                .with_context(|| format!("naming {}", run.display()))?
                .components()
                .map(|component| component.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            let watches = parse_output(&text, root.as_deref());
            units.push(Unit {
                key,
                output,
                root,
                watches,
            });
        }
    }
    Ok(units)
}

/// Each workspace package's root, by package name.
pub(super) fn workspace_packages(root: &Path) -> Result<BTreeMap<String, PathBuf>> {
    let metadata = cargo_metadata::MetadataCommand::new()
        .current_dir(root)
        .no_deps()
        .exec()
        .context("describing the workspace's packages")?;
    Ok(metadata
        .packages
        .into_iter()
        .filter_map(|package| {
            let root = package.manifest_path.parent()?.as_std_path().to_path_buf();
            Some((package.name.to_string(), root))
        })
        .collect())
}

/// What a run was run against, or `None` when the claim cannot name it: a
/// workspace script that declares nothing reruns on any change to its
/// package, and a watched checkout path git does not track has no content.
/// A dependency's run keeps an empty record; its registry sources are fixed.
fn record_of(unit: &Unit, checkout: &Checkout) -> Result<Option<UnitRecord>> {
    let output_modified = modified(&unit.output)?;
    let mut watched = BTreeMap::new();
    if unit.is_workspace() {
        if !unit.watches.declares {
            return Ok(None);
        }
        for path in &unit.watches.paths {
            if let Place::Checkout(name) = checkout.classify(path)? {
                let Some(oid) = tracked::oid(&checkout.tracked, &name) else {
                    return Ok(None);
                };
                watched.insert(name, oid);
            }
        }
    }
    Ok(Some(UnitRecord {
        output_modified,
        watched,
    }))
}

/// Whether `path`, or anything under it, is newer than `reference`. A path
/// that is gone counts as changed, as it does for cargo.
fn newer(path: &Path, reference: SystemTime) -> Result<bool> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    if metadata.modified()? > reference {
        return Ok(true);
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path).with_context(|| format!("listing {}", path.display()))? {
            if newer(&entry?.path(), reference)? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Whether a run watches a path whose content nothing names, in the lane
/// directory or outside both, that changed since the run; its mtime decides,
/// as it would for cargo.
fn outside_changed(unit: &Unit, checkout: &Checkout, reference: SystemTime) -> Result<bool> {
    for path in &unit.watches.paths {
        let changed = match checkout.classify(path)? {
            Place::Checkout(_) => false,
            Place::Build => newer(path, reference)?,
            Place::Outside(resolved) => newer(&resolved, reference)?,
        };
        if changed {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Sets every artifact under the lane's profiles to `at`, so no dependency
/// reads newer than a unit depending on it. Cargo's own records
/// (`.fingerprint`, `incremental`) and what build scripts wrote (`out`) keep
/// their mtimes, and a link is never followed.
fn align(dir: &Path, at: SystemTime) -> Result<()> {
    for profile in profiles(dir)? {
        for entry in
            fs::read_dir(&profile).with_context(|| format!("listing {}", profile.display()))?
        {
            let path = entry?.path();
            match path.file_name().and_then(OsStr::to_str) {
                Some(".fingerprint" | "incremental") => {}
                Some("build") => {
                    for run in subdirectories(&path)? {
                        for entry in fs::read_dir(&run)
                            .with_context(|| format!("listing {}", run.display()))?
                        {
                            let path = entry?.path();
                            if path.file_name() != Some(OsStr::new("out")) {
                                touch_tree(&path, at)?;
                            }
                        }
                    }
                }
                _ => touch_tree(&path, at)?,
            }
        }
    }
    Ok(())
}

/// Sets `path`, and every file under it, to `at`, never following a link.
fn touch_tree(path: &Path, at: SystemTime) -> Result<()> {
    let metadata =
        fs::symlink_metadata(path).with_context(|| format!("reading {}", path.display()))?;
    if metadata.is_symlink() {
        return Ok(());
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path).with_context(|| format!("listing {}", path.display()))? {
            touch_tree(&entry?.path(), at)?;
        }
        return Ok(());
    }
    File::options()
        .write(true)
        .open(path)
        .and_then(|file| file.set_modified(at))
        .with_context(|| format!("aligning {}", path.display()))
}

fn read_records(path: &Path) -> Result<Records> {
    match fs::read(path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Records::new()),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

fn write_records(path: &Path, records: &Records) -> Result<()> {
    let partial = path.with_extension("partial");
    fs::write(&partial, serde_json::to_vec(records)?)
        .with_context(|| format!("writing {}", partial.display()))?;
    fs::rename(&partial, path).with_context(|| format!("replacing {}", path.display()))
}

/// A record of every run in the lane directory the claim can name.
pub(super) fn record_all(
    checkout: &Checkout,
    packages: &BTreeMap<String, PathBuf>,
) -> Result<Records> {
    let mut records = Records::new();
    for unit in discover(&checkout.build, packages)? {
        if let Some(record) = record_of(&unit, checkout)? {
            records.insert(unit.key, record);
        }
    }
    Ok(records)
}

/// Keeps each run whose record still holds and whose unnamed watches did not
/// change since, and removes every other run's `output`, so cargo runs that
/// script again. Then aligns the directory to `at` and returns the kept runs'
/// records as alignment left them.
pub(super) fn keep_fresh(
    checkout: &Checkout,
    units: &[Unit],
    records: &Records,
    at: SystemTime,
) -> Result<Records> {
    let mut kept = Vec::new();
    for unit in units {
        let current = record_of(unit, checkout)?;
        let fresh = if let (Some(current), Some(recorded)) = (&current, records.get(&unit.key)) {
            current == recorded && !outside_changed(unit, checkout, current.output_modified)?
        } else {
            false
        };
        if fresh {
            kept.push(unit);
        } else {
            fs::remove_file(&unit.output)
                .with_context(|| format!("removing {}", unit.output.display()))?;
        }
    }
    align(&checkout.build, at)?;
    let mut records = Records::new();
    for unit in kept {
        if let Some(record) = record_of(unit, checkout)? {
            records.insert(unit.key.clone(), record);
        }
    }
    Ok(records)
}

/// What a checksum lane's claim needs again at its settle.
pub(super) struct Claimed {
    checkout: Checkout,
    packages: BTreeMap<String, PathBuf>,
}

/// Decides every run in the lane directory against the checkout, aligns the
/// directory to `at`, and records only the runs it kept, so a job that dies
/// leaves no record of a run it may have changed.
pub(super) fn claim(
    project_root: &Path,
    dir: &Path,
    tracked: Sources,
    at: SystemTime,
) -> Result<Claimed> {
    let checkout = Checkout::new(project_root, dir, tracked)?;
    let packages = workspace_packages(&checkout.root)?;
    let units = discover(&checkout.build, &packages)?;
    let record = checkout.build.join(consts::UNITS_FILE);
    let kept = keep_fresh(&checkout, &units, &read_records(&record)?, at)?;
    write_records(&record, &kept)?;
    Ok(Claimed { checkout, packages })
}

impl Claimed {
    /// Records every run the job left, whether or not it succeeded: cargo
    /// writes a run's `output` only once its script has run to the end.
    pub(super) fn settle(&self) -> Result<()> {
        write_records(
            &self.checkout.build.join(consts::UNITS_FILE),
            &record_all(&self.checkout, &self.packages)?,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::{env, time::Duration};

    use super::*;
    use crate::{
        ci::lane_build::{
            LaneBuild,
            fixture::{cargo_checkout, git, mtime, pool, set_mtime, slot, write_unit},
        },
        config::LaneFreshness,
        consts::{DAY, PROBE_BUILD_RUN, PROBE_DIRECTIVE},
    };

    /// Claims the first slot under the fleet root `lanes` as a checksum lane.
    fn claim_slot(checkout: &Path, lanes: &Path) -> LaneBuild {
        LaneBuild::claim(checkout, &pool(lanes), DAY, LaneFreshness::Checksum).unwrap()
    }

    /// Claims the slot, lets `build` stand for the job's build, and settles it.
    fn run_lane(checkout: &Path, lanes: &Path, build: impl FnOnce()) {
        let claim = claim_slot(checkout, lanes);
        build();
        claim.settle(true).unwrap();
    }

    #[test]
    fn directives_name_what_cargo_watches() {
        let model = env::temp_dir().join("model");
        let text = format!(
            "cargo::rerun-if-changed=build.rs\ncargo:rerun-if-changed={}\n\
             cargo::rerun-if-env-changed=CACHE\ncargo::rustc-env=MODEL=x\n",
            model.display()
        );

        assert_eq!(
            parse_output(&text, Some(Path::new("/c/crates/probe"))),
            Watches {
                paths: vec![PathBuf::from("/c/crates/probe/build.rs"), model.clone()],
                declares: true,
            }
        );
        assert_eq!(
            parse_output("cargo::rerun-if-env-changed=CACHE\n", None),
            Watches {
                paths: Vec::new(),
                declares: true,
            }
        );
        assert_eq!(
            parse_output(&text, None),
            Watches {
                paths: vec![model],
                declares: true,
            },
            "a dependency's relative path names its own fixed sources"
        );
        assert_eq!(
            parse_output("cargo::rustc-cfg=probe\n", None),
            Watches::default()
        );
    }

    /// The first checksum claim on a slot an mtime lane built has no record
    /// of any run, so every script runs once more and nothing fails; from
    /// then on the recorded runs are kept.
    #[test]
    fn a_directory_no_checksum_claim_recorded_reruns_every_script_once() {
        let checkout = cargo_checkout(&[]);
        let lanes = tempfile::tempdir().unwrap();
        let lane = slot(lanes.path());
        let workspace = write_unit(&lane, PROBE_BUILD_RUN, PROBE_DIRECTIVE);
        let dependency = write_unit(
            &lane,
            "debug/build/serde-0123456789abcdef",
            "cargo::rustc-cfg=std\n",
        );

        run_lane(checkout.path(), lanes.path(), || {
            assert!(!workspace.exists(), "an unrecorded run reruns");
            assert!(!dependency.exists(), "an unrecorded run reruns");
            write_unit(&lane, PROBE_BUILD_RUN, PROBE_DIRECTIVE);
        });
        let _claim = claim_slot(checkout.path(), lanes.path());

        assert!(workspace.exists(), "a recorded run is kept");
    }

    #[test]
    fn a_file_added_or_removed_under_a_watched_directory_reruns_the_unit() {
        let checkout = cargo_checkout(&[("assets/a.bin", "a")]);
        let lanes = tempfile::tempdir().unwrap();
        let lane = slot(lanes.path());
        let directive = "cargo::rerun-if-changed=assets\n";
        let output = lane.join(PROBE_BUILD_RUN).join("output");

        run_lane(checkout.path(), lanes.path(), || {
            write_unit(&lane, PROBE_BUILD_RUN, directive);
        });
        run_lane(checkout.path(), lanes.path(), || {});
        assert!(output.exists(), "an unchanged directory keeps the run");

        fs::write(checkout.path().join("assets/b.bin"), "b").unwrap();
        git(checkout.path(), &["add", "assets/b.bin"]);
        run_lane(checkout.path(), lanes.path(), || {
            assert!(!output.exists(), "an added file reruns the script");
            write_unit(&lane, PROBE_BUILD_RUN, directive);
        });

        git(checkout.path(), &["rm", "-q", "-f", "assets/a.bin"]);
        let _claim = claim_slot(checkout.path(), lanes.path());
        assert!(!output.exists(), "a removed file reruns the script");
    }

    /// A registry package's sources never change under one version, so cargo's
    /// own judgement of a dependency's run holds.
    #[test]
    fn a_dependency_build_script_that_declares_nothing_keeps_its_output() {
        let checkout = cargo_checkout(&[]);
        let lanes = tempfile::tempdir().unwrap();
        let lane = slot(lanes.path());
        let key = "debug/build/serde-0123456789abcdef";

        run_lane(checkout.path(), lanes.path(), || {
            write_unit(&lane, key, "cargo::rustc-cfg=std\n");
        });
        let _claim = claim_slot(checkout.path(), lanes.path());

        assert!(lane.join(key).join("output").exists());
    }

    /// Cargo reruns a script that names nothing whenever anything in its
    /// package is newer than its run; the claim cannot name what such a run
    /// read, so it drops the run rather than guess.
    #[test]
    fn a_workspace_build_script_that_declares_nothing_reruns_every_claim() {
        let checkout = cargo_checkout(&[]);
        let lanes = tempfile::tempdir().unwrap();
        let lane = slot(lanes.path());

        run_lane(checkout.path(), lanes.path(), || {
            write_unit(&lane, PROBE_BUILD_RUN, "cargo::rustc-cfg=probe\n");
        });
        let _claim = claim_slot(checkout.path(), lanes.path());

        assert!(!lane.join(PROBE_BUILD_RUN).join("output").exists());
    }

    /// Nothing names the content of a path outside the checkout, so its mtime
    /// decides, as it would for cargo; a step's nested profile is searched.
    #[test]
    fn a_path_outside_the_checkout_is_judged_by_its_mtime() {
        let checkout = cargo_checkout(&[]);
        let lanes = tempfile::tempdir().unwrap();
        let lane = slot(lanes.path());
        let elsewhere = tempfile::tempdir().unwrap();
        let model = elsewhere.path().join("model.onnx");
        fs::write(&model, "weights").unwrap();
        set_mtime(&model, SystemTime::UNIX_EPOCH);
        let key = format!("ci-tests-flash-off/{PROBE_BUILD_RUN}");
        let directive = format!("cargo::rerun-if-changed={}\n", model.display());
        let output = lane.join(&key).join("output");

        run_lane(checkout.path(), lanes.path(), || {
            write_unit(&lane, &key, &directive);
        });
        run_lane(checkout.path(), lanes.path(), || {});
        assert!(output.exists(), "an unchanged outside path keeps the run");

        set_mtime(&model, SystemTime::now() + Duration::from_secs(3600));
        let _claim = claim_slot(checkout.path(), lanes.path());
        assert!(!output.exists(), "a newer outside path reruns the script");
    }

    /// Cargo reads the worktree, so an edit nobody staged reruns a script
    /// that watches the file, and the next claim keeps the run it made.
    #[test]
    fn an_unstaged_edit_to_a_watched_file_reruns_the_script() {
        let checkout = cargo_checkout(&[]);
        let lanes = tempfile::tempdir().unwrap();
        let lane = slot(lanes.path());
        let output = lane.join(PROBE_BUILD_RUN).join("output");
        run_lane(checkout.path(), lanes.path(), || {
            write_unit(&lane, PROBE_BUILD_RUN, PROBE_DIRECTIVE);
        });

        fs::write(checkout.path().join("src/lib.rs"), "pub fn edited() {}").unwrap();
        run_lane(checkout.path(), lanes.path(), || {
            assert!(!output.exists(), "an unstaged edit reruns the script");
            write_unit(&lane, PROBE_BUILD_RUN, PROBE_DIRECTIVE);
        });
        let _claim = claim_slot(checkout.path(), lanes.path());

        assert!(output.exists(), "the run made from the edit is kept");
    }

    /// A dependency's own sources are fixed by its version, but a path it
    /// names outside them, such as a system library a `-sys` script links,
    /// is judged by its mtime as it would be for cargo.
    #[test]
    fn a_dependency_run_is_judged_by_what_it_names_outside_its_sources() {
        let checkout = cargo_checkout(&[]);
        let lanes = tempfile::tempdir().unwrap();
        let lane = slot(lanes.path());
        let elsewhere = tempfile::tempdir().unwrap();
        let library = elsewhere.path().join("libsystem.so");
        fs::write(&library, "v1").unwrap();
        set_mtime(&library, SystemTime::UNIX_EPOCH);
        let key = "debug/build/system-sys-0123456789abcdef";
        let directive = format!(
            "cargo::rerun-if-changed=build.rs\ncargo::rerun-if-changed={}\n",
            library.display()
        );
        let output = lane.join(key).join("output");

        run_lane(checkout.path(), lanes.path(), || {
            write_unit(&lane, key, &directive);
        });
        run_lane(checkout.path(), lanes.path(), || {});
        assert!(output.exists(), "an unchanged system path keeps the run");

        set_mtime(&library, SystemTime::now() + Duration::from_secs(3600));
        let _claim = claim_slot(checkout.path(), lanes.path());
        assert!(!output.exists(), "a newer system path reruns the script");
    }

    /// A job that died after its script ran again leaves an `output` no
    /// settle recorded; the next claim cannot tell what that run read.
    #[test]
    fn an_output_an_unsettled_job_rewrote_is_dropped() {
        let checkout = cargo_checkout(&[]);
        let lanes = tempfile::tempdir().unwrap();
        let lane = slot(lanes.path());
        let output = lane.join(PROBE_BUILD_RUN).join("output");
        run_lane(checkout.path(), lanes.path(), || {
            write_unit(&lane, PROBE_BUILD_RUN, PROBE_DIRECTIVE);
        });

        let dead = claim_slot(checkout.path(), lanes.path());
        fs::write(&output, PROBE_DIRECTIVE).unwrap();
        set_mtime(&output, SystemTime::now() + Duration::from_secs(3600));
        drop(dead);

        let _claim = claim_slot(checkout.path(), lanes.path());
        assert!(!output.exists());
    }

    /// Other lanes on the same runner read the checkout's mtimes, so a
    /// checksum claim writes the slot alone, even where an mtime claim of a
    /// slot with no record would stamp every file.
    #[test]
    fn a_checksum_claim_never_writes_the_checkout() {
        let checkout = cargo_checkout(&[]);
        let lanes = tempfile::tempdir().unwrap();
        let lib = checkout.path().join("src/lib.rs");
        set_mtime(&lib, SystemTime::UNIX_EPOCH);

        run_lane(checkout.path(), lanes.path(), || {});

        assert_eq!(mtime(&lib), SystemTime::UNIX_EPOCH);
    }

    /// Every artifact a kept run's dependents read is set to one claim
    /// instant, so none reads newer than a unit depending on it; cargo's own
    /// records and what scripts generated keep their mtimes.
    #[test]
    fn a_kept_unit_is_aligned_to_the_claim() {
        let checkout = cargo_checkout(&[]);
        let lanes = tempfile::tempdir().unwrap();
        let lane = slot(lanes.path());
        let rlib = lane.join("debug/deps/libprobe-0123456789abcdef.rlib");
        let fingerprint = lane.join("debug/.fingerprint/probe-0123456789abcdef/lib-probe");
        let generated = lane.join(PROBE_BUILD_RUN).join("out/generated.rs");
        let output = lane.join(PROBE_BUILD_RUN).join("output");
        run_lane(checkout.path(), lanes.path(), || {
            write_unit(&lane, PROBE_BUILD_RUN, PROBE_DIRECTIVE);
            for file in [&rlib, &fingerprint, &generated] {
                fs::create_dir_all(file.parent().unwrap()).unwrap();
                fs::write(file, "artifact").unwrap();
            }
            for file in [&rlib, &fingerprint, &generated, &output] {
                set_mtime(file, SystemTime::UNIX_EPOCH);
            }
        });

        let _claim = claim_slot(checkout.path(), lanes.path());

        assert!(output.exists(), "the run is kept");
        assert!(
            mtime(&output) > SystemTime::UNIX_EPOCH,
            "the run is aligned"
        );
        assert_eq!(mtime(&rlib), mtime(&output));
        assert_eq!(mtime(&fingerprint), SystemTime::UNIX_EPOCH);
        assert_eq!(mtime(&generated), SystemTime::UNIX_EPOCH);
    }

    #[cfg(unix)]
    #[test]
    fn align_never_follows_a_link_out_of_the_lane() {
        let lane = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let file = outside.path().join("file");
        let inner = outside.path().join("dir/inner");
        fs::create_dir_all(inner.parent().unwrap()).unwrap();
        fs::write(&file, "x").unwrap();
        fs::write(&inner, "x").unwrap();
        set_mtime(&file, SystemTime::UNIX_EPOCH);
        set_mtime(&inner, SystemTime::UNIX_EPOCH);
        fs::create_dir_all(lane.path().join("debug/.fingerprint")).unwrap();
        fs::create_dir_all(lane.path().join("debug/deps")).unwrap();
        std::os::unix::fs::symlink(&file, lane.path().join("debug/deps/linked")).unwrap();
        std::os::unix::fs::symlink(inner.parent().unwrap(), lane.path().join("debug/linked"))
            .unwrap();

        align(lane.path(), SystemTime::now()).unwrap();

        assert_eq!(mtime(&file), SystemTime::UNIX_EPOCH);
        assert_eq!(mtime(&inner), SystemTime::UNIX_EPOCH);
    }
}
