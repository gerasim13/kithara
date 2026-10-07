use std::{
    ffi::OsStr,
    fs, io,
    path::{Component, Path, PathBuf},
    process,
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use kithara_devtools::lease::{self, Lease};
use tracing::warn;

use super::sources::{Claim, claim};
use crate::{ci::build_cache, consts};

/// A job's hold on its build directory: leased, with the root's alias naming
/// it and the checkout claimed for it, for as long as this lives. The claim
/// settles before the lease goes.
#[derive(Debug, fieldwork::Fieldwork)]
#[fieldwork(opt_in, get, deref = false)]
pub(crate) struct BuildDir {
    #[field(get, vis = "pub(crate)")]
    path: PathBuf,
    _sources: Claim,
    _lease: Lease,
}

impl BuildDir {
    /// Enters build `id` under the root `alias` sits in: leases `<root>/<id>`,
    /// points the alias at it, removes the units its builds no longer ask
    /// for, by `window` (see [`super::garbage`]), and claims `checkout` for
    /// it (see [`super::sources`]).
    ///
    /// # Errors
    ///
    /// When `alias` is not named `build`, `id` is not one plain name the root
    /// leaves free, or the lease, the link or the claim cannot be made. A
    /// garbage pass that fails is logged: it only saves disk.
    pub(crate) fn enter(checkout: &Path, alias: &Path, id: &str, window: Duration) -> Result<Self> {
        ensure!(
            alias.file_name() == Some(OsStr::new(consts::BUILD_ALIAS)),
            "{} names no build alias: an executor names `<root>/{}` and the lane builds behind it",
            alias.display(),
            consts::BUILD_ALIAS
        );
        let root = alias
            .parent()
            .with_context(|| format!("{} has no build root", alias.display()))?;
        validate(id)?;
        let path = root.join(id);
        let lease =
            lease::hold(&path).with_context(|| format!("leasing build {}", path.display()))?;
        point(alias, id)?;
        if let Err(error) = super::garbage::collect(&path, window) {
            warn!(
                "{error:#}; build {} keeps every unit it holds",
                path.display()
            );
        }
        let sources = claim(checkout, &path)?;
        Ok(Self {
            path,
            _sources: sources,
            _lease: lease,
        })
    }
}

/// A build id is one plain name, neither hidden - an eviction in progress and
/// a staged link are - nor one the root keeps for itself.
fn validate(id: &str) -> Result<()> {
    let plain = matches!(
        Path::new(id).components().collect::<Vec<_>>().as_slice(),
        [Component::Normal(name)] if *name == OsStr::new(id)
    );
    ensure!(
        plain && !id.starts_with('.') && id != consts::BUILD_ALIAS && id != consts::XTASK_BUILD,
        "`{id}` is not a build id: one plain name, not hidden, and neither `{}` nor `{}`",
        consts::BUILD_ALIAS,
        consts::XTASK_BUILD
    );
    Ok(())
}

/// Points `alias` at `id` beside it. The new link is staged under a hidden
/// name and renamed over the old one, so a reader sees one link or the other.
fn point(alias: &Path, id: &str) -> Result<()> {
    match fs::symlink_metadata(alias) {
        Ok(metadata) if metadata.file_type().is_symlink() => {}
        // Cargo made a build here for a job that named the alias as its
        // directory without entering one. The root's directories are the
        // build cache's, so it leaves the way an evicted build does.
        Ok(_) => {
            let aside = build_cache::aside(alias)?;
            fs::rename(alias, &aside)
                .with_context(|| format!("moving the build at {} aside", alias.display()))?;
            warn!(
                "{} was a build, not a link; moved aside to {} for the build cache to remove",
                alias.display(),
                aside.display()
            );
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("reading {}", alias.display()));
        }
    }
    let staged = alias.with_file_name(format!(".{}-{}", consts::BUILD_ALIAS, process::id()));
    match fs::remove_file(&staged) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("removing {}", staged.display()));
        }
    }
    link(Path::new(id), &staged).with_context(|| format!("linking {}", staged.display()))?;
    fs::rename(&staged, alias).with_context(|| format!("pointing {} at {id}", alias.display()))
}

#[cfg(unix)]
fn link(target: &Path, at: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, at)
}

#[cfg(windows)]
fn link(target: &Path, at: &Path) -> io::Result<()> {
    std::os::windows::fs::symlink_dir(target, at)
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use kithara_devtools::lease;
    use tempfile::TempDir;

    use super::BuildDir;
    use crate::{
        ci::{build_cache, build_dir::fixture::git_checkout},
        consts,
    };

    fn alias(root: &Path) -> std::path::PathBuf {
        root.join("build")
    }

    /// Enters build `id` for a checkout of its own.
    fn enter(alias: &Path, id: &str) -> anyhow::Result<(BuildDir, TempDir)> {
        let checkout = git_checkout(&[]);
        BuildDir::enter(checkout.path(), alias, id, consts::GARBAGE_WINDOW)
            .map(|build| (build, checkout))
    }

    #[test]
    fn entering_points_the_alias_at_the_build_directory() {
        let root = tempfile::tempdir().unwrap();

        let (build, _checkout) = enter(&alias(root.path()), "lint").unwrap();

        assert_eq!(build.path(), &root.path().join("lint"));
        assert_eq!(
            fs::read_link(alias(root.path())).unwrap(),
            Path::new("lint")
        );
        assert!(root.path().join("lint").join(lease::FILE).is_file());
    }

    #[test]
    fn the_next_build_re_points_the_alias_and_keeps_the_last_one() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("lint/debug")).unwrap();
        drop(enter(&alias(root.path()), "lint").unwrap());

        let (usdt, _checkout) = enter(&alias(root.path()), "usdt").unwrap();

        assert_eq!(
            fs::read_link(alias(root.path())).unwrap(),
            Path::new("usdt")
        );
        assert!(root.path().join("lint/debug").is_dir());
        drop(usdt);
    }

    #[test]
    fn an_entered_build_directory_is_leased() {
        let root = tempfile::tempdir().unwrap();

        let (build, _checkout) = enter(&alias(root.path()), "lint").unwrap();

        assert!(lease::evict(build.path()).unwrap().is_none());
    }

    /// A directory standing where the alias goes is a build Cargo made for a
    /// job that named the alias as its directory without entering a build.
    /// The root's directories are the build cache's, so it leaves the way an
    /// evicted build does, and the next lane still enters its own.
    #[test]
    fn a_directory_at_the_alias_is_moved_aside_for_the_build_cache_to_remove() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(alias(root.path()).join("debug")).unwrap();

        let (build, _checkout) = enter(&alias(root.path()), "lint").unwrap();

        assert_eq!(
            fs::read_link(alias(root.path())).unwrap(),
            Path::new("lint")
        );
        let moved: Vec<_> = fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.join("debug").is_dir())
            .collect();
        let [moved] = moved.as_slice() else {
            panic!("the build at the alias is moved, not copied or lost: {moved:?}");
        };
        drop(build);
        build_cache::enforce_budget(&[root.path().to_path_buf()], u64::MAX).unwrap();
        assert!(!moved.exists(), "{} is left behind", moved.display());
    }

    #[test]
    fn a_build_id_is_one_plain_name_the_root_does_not_reserve() {
        let root = tempfile::tempdir().unwrap();
        for id in [
            "",
            "build",
            "xtask",
            ".evicting-lint",
            "lint/flash-off",
            "..",
        ] {
            assert!(
                enter(&alias(root.path()), id).is_err(),
                "`{id}` is not a build id"
            );
        }
    }

    #[test]
    fn an_alias_is_named_build() {
        let root = tempfile::tempdir().unwrap();

        let error = enter(&root.path().join("target"), "lint").unwrap_err();

        assert!(format!("{error:#}").contains("build"), "{error:#}");
    }
}
