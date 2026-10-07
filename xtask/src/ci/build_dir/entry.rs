use std::{
    ffi::OsStr,
    fs, io,
    path::{Component, Path, PathBuf},
    process,
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use kithara_devtools::lease::{self, Lease};
use tracing::warn;

use crate::consts;

/// A job's hold on its build directory: leased, with the root's alias naming
/// it, for as long as this lives.
#[derive(Debug, fieldwork::Fieldwork)]
#[fieldwork(opt_in, get, deref = false)]
pub(crate) struct BuildDir {
    #[field(get, vis = "pub(crate)")]
    path: PathBuf,
    _lease: Lease,
}

impl BuildDir {
    /// Enters build `id` under the root `alias` sits in: leases `<root>/<id>`,
    /// points the alias at it and removes the units its builds no longer ask
    /// for, by `window` (see [`super::garbage`]).
    ///
    /// # Errors
    ///
    /// When `alias` is not named `build`, `id` is not one plain name the root
    /// leaves free, a directory stands at the alias, or the lease or the link
    /// cannot be made. A garbage pass that fails is logged: it only saves
    /// disk.
    pub(crate) fn enter(alias: &Path, id: &str, window: Duration) -> Result<Self> {
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
        Ok(Self {
            path,
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
        Ok(_) => bail!(
            "{} is not a link but a build someone made there; move it away once, and the alias \
             names each job's build from then on",
            alias.display()
        ),
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

    use super::BuildDir;
    use crate::consts;

    fn alias(root: &Path) -> std::path::PathBuf {
        root.join("build")
    }

    #[test]
    fn entering_points_the_alias_at_the_build_directory() {
        let root = tempfile::tempdir().unwrap();

        let build = BuildDir::enter(&alias(root.path()), "lint", consts::GARBAGE_WINDOW).unwrap();

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
        drop(BuildDir::enter(&alias(root.path()), "lint", consts::GARBAGE_WINDOW).unwrap());

        let usdt = BuildDir::enter(&alias(root.path()), "usdt", consts::GARBAGE_WINDOW).unwrap();

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

        let build = BuildDir::enter(&alias(root.path()), "lint", consts::GARBAGE_WINDOW).unwrap();

        assert!(lease::evict(build.path()).unwrap().is_none());
    }

    /// A directory standing where the alias goes is a build someone made
    /// there; replacing it would delete it.
    #[test]
    fn a_directory_at_the_alias_is_refused_and_kept() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(alias(root.path()).join("debug")).unwrap();

        let error =
            BuildDir::enter(&alias(root.path()), "lint", consts::GARBAGE_WINDOW).unwrap_err();

        assert!(format!("{error:#}").contains("not a link"), "{error:#}");
        assert!(alias(root.path()).join("debug").is_dir());
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
                BuildDir::enter(&alias(root.path()), id, consts::GARBAGE_WINDOW).is_err(),
                "`{id}` is not a build id"
            );
        }
    }

    #[test]
    fn an_alias_is_named_build() {
        let root = tempfile::tempdir().unwrap();

        let error = BuildDir::enter(&root.path().join("target"), "lint", consts::GARBAGE_WINDOW)
            .unwrap_err();

        assert!(format!("{error:#}").contains("build"), "{error:#}");
    }
}
