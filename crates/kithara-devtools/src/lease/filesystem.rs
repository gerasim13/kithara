use std::{
    ffi::{OsStr, OsString},
    fs::{self, File},
    io::{self, Read as _, Write as _},
    os::unix::{ffi::OsStrExt as _, fs::MetadataExt as _},
    path::{self, Component, Path, PathBuf},
    process,
    time::{SystemTime, UNIX_EPOCH},
};

use nix::{
    dir::Dir,
    fcntl::{AtFlags, OFlag, openat, renameat},
    sys::stat::{FileStat, Mode, fstat, fstatat, mkdirat},
    unistd::{UnlinkatFlags, unlinkat},
};

use crate::lock::{
    FileLock,
    filesystem::{DIRECTORY, descend, open_directory, unique_file},
};

/// An entry's identity captured beneath an owned root without following links.
/// The caller retains its exclusive ownership guard through removal.
#[derive(Debug)]
pub struct Candidate {
    root: File,
    parent: File,
    directory: File,
    root_path: PathBuf,
    relative: PathBuf,
}

impl Candidate {
    /// Captures a regular file or directory beneath `root` without following links.
    ///
    /// # Errors
    /// Unsafe components, symlinks, missing directories, or filesystem errors.
    pub fn open(root: &Path, relative: &Path) -> io::Result<Self> {
        if relative.as_os_str().is_empty()
            || !relative
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "candidate must be a normal relative path",
            ));
        }
        let root_path = path::absolute(root)?;
        let root = open_directory(&root_path)?;
        let parent = descend(&root, relative.parent().unwrap_or_else(|| Path::new("")))?;
        let name = relative
            .file_name()
            .ok_or_else(|| io::Error::other("candidate needs a name"))?;
        let directory = File::from(openat(
            &parent,
            name,
            OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
            Mode::empty(),
        )?);
        if !directory.metadata()?.is_dir() && !directory.metadata()?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "candidate must be a file or directory",
            ));
        }
        Ok(Self {
            root,
            parent,
            directory,
            root_path,
            relative: relative.to_owned(),
        })
    }

    /// Metadata of the original inspected entry, independent of later path replacement.
    ///
    /// # Errors
    /// When the captured descriptor cannot be inspected.
    pub fn metadata(&self) -> io::Result<fs::Metadata> {
        self.directory.metadata()
    }

    /// Opens the original file through its captured parent, refusing a replacement.
    ///
    /// # Errors
    /// Identity changes, multiply linked or nonregular files, or filesystem errors.
    pub fn open_file(&self, write: bool) -> io::Result<File> {
        let name = self
            .relative
            .file_name()
            .ok_or_else(|| io::Error::other("candidate needs a name"))?;
        let access = if write {
            OFlag::O_RDWR
        } else {
            OFlag::O_RDONLY
        };
        let file = File::from(openat(
            &self.parent,
            name,
            access | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
            Mode::empty(),
        )?);
        if !same(&fstat(&file)?, &fstat(&self.directory)?) {
            return Err(io::Error::other("candidate file changed identity"));
        }
        unique_file(&file)?;
        Ok(file)
    }

    /// Opens or creates a regular sibling file through the captured parent.
    ///
    /// # Errors
    /// Non-normal names, symlinks, hardlinks, nonregular entries, or filesystem errors.
    pub fn open_sibling(&self, name: &OsStr) -> io::Result<File> {
        crate::lock::open_at(&self.parent, name)
    }

    /// Removes the captured directory, or retains a changed candidate and returns false.
    /// A replacement racing with rename stays in the private eviction container.
    /// Interrupted containers retain their lease for a subsequent fenced sweep.
    ///
    /// # Errors
    /// Filesystem uncertainty or failure to remove the original candidate.
    pub fn remove(&self) -> io::Result<bool> {
        if self.directory.metadata()?.is_dir()
            && let Some(expected) = interrupted_identity(&self.directory)?
        {
            match openat(
                &self.directory,
                "data",
                OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
                Mode::empty(),
            ) {
                Ok(data) => {
                    let found = File::from(data).metadata()?;
                    if (found.dev(), found.ino()) != expected {
                        return Ok(false);
                    }
                }
                Err(nix::errno::Errno::ENOENT) => {}
                Err(error) => return Err(error.into()),
            }
        }
        let root = open_directory(&self.root_path)?;
        if !same(&fstat(&root)?, &fstat(&self.root)?) {
            return Ok(false);
        }
        let parent = descend(
            &root,
            self.relative.parent().unwrap_or_else(|| Path::new("")),
        )?;
        if !same(&fstat(&parent)?, &fstat(&self.parent)?) {
            return Ok(false);
        }
        let name = self
            .relative
            .file_name()
            .ok_or_else(|| io::Error::other("candidate needs a name"))?;
        let expected = fstat(&self.directory)?;
        if !same(
            &fstatat(&parent, name, AtFlags::AT_SYMLINK_NOFOLLOW)?,
            &expected,
        ) {
            return Ok(false);
        }
        let at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        let aside = OsString::from(format!(".evicting-{}-{at}", process::id()));
        mkdirat(&parent, aside.as_os_str(), Mode::S_IRWXU)?;
        let container = File::from(openat(
            &parent,
            aside.as_os_str(),
            DIRECTORY,
            Mode::empty(),
        )?);
        let mut marker = File::from(openat(
            &container,
            super::FILE,
            OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_RDWR | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
            Mode::S_IRUSR | Mode::S_IWUSR,
        )?);
        let identity = self.directory.metadata()?;
        writeln!(marker, "eviction={}:{}", identity.dev(), identity.ino())?;
        marker.sync_data()?;
        let _removing = FileLock::try_exclusive(marker).map_err(io::Error::other)?;
        renameat(&parent, name, &container, "data")?;
        if !same(
            &fstatat(&container, "data", AtFlags::AT_SYMLINK_NOFOLLOW)?,
            &expected,
        ) {
            return Ok(false);
        }
        let removal = if self.directory.metadata()?.is_dir() {
            clear(&self.directory, true)?;
            UnlinkatFlags::RemoveDir
        } else {
            UnlinkatFlags::NoRemoveDir
        };
        if !same(
            &fstatat(&container, "data", AtFlags::AT_SYMLINK_NOFOLLOW)?,
            &expected,
        ) {
            return Ok(false);
        }
        unlinkat(&container, "data", removal)?;
        unlinkat(&container, super::FILE, UnlinkatFlags::NoRemoveDir)?;
        unlinkat(&parent, aside.as_os_str(), UnlinkatFlags::RemoveDir)?;
        Ok(true)
    }
}

fn same(left: &FileStat, right: &FileStat) -> bool {
    left.st_dev == right.st_dev && left.st_ino == right.st_ino
}

fn interrupted_identity(directory: &File) -> io::Result<Option<(u64, u64)>> {
    let marker = match openat(
        directory,
        super::FILE,
        OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
        Mode::empty(),
    ) {
        Ok(marker) => File::from(marker),
        Err(nix::errno::Errno::ENOENT) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !marker.metadata()?.is_file() {
        return Err(io::Error::other("eviction marker must be regular"));
    }
    let mut record = String::new();
    marker.take(128).read_to_string(&mut record)?;
    let Some(identity) = record.strip_prefix("eviction=") else {
        return Ok(None);
    };
    let (dev, ino) = identity
        .trim()
        .split_once(':')
        .ok_or_else(|| io::Error::other("invalid eviction identity"))?;
    Ok(Some((
        dev.parse().map_err(io::Error::other)?,
        ino.parse().map_err(io::Error::other)?,
    )))
}

fn nested_fence(directory: &File) -> io::Result<Option<FileLock>> {
    let marker = match openat(
        directory,
        super::FILE,
        OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
        Mode::empty(),
    ) {
        Ok(marker) => File::from(marker),
        Err(nix::errno::Errno::ENOENT) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !marker.metadata()?.is_file() {
        return Err(io::Error::other("nested lease must be regular"));
    }
    let identity = fstat(&marker)?;
    let lock = FileLock::try_exclusive(marker).map_err(|error| match error {
        fs4::TryLockError::WouldBlock => io::Error::new(
            io::ErrorKind::WouldBlock,
            "nested eviction or job still owns this directory",
        ),
        fs4::TryLockError::Error(error) => error,
    })?;
    if !same(
        &identity,
        &fstatat(directory, super::FILE, AtFlags::AT_SYMLINK_NOFOLLOW)?,
    ) {
        return Err(io::Error::other("nested lease changed identity"));
    }
    Ok(Some(lock))
}

fn clear(directory: &File, guarded: bool) -> io::Result<()> {
    let _nested = if guarded {
        None
    } else {
        nested_fence(directory)?
    };
    let interrupted = interrupted_identity(directory)?;
    let mut listing = Dir::from_fd(openat(directory, ".", DIRECTORY, Mode::empty())?)?;
    let mut children = Vec::new();
    for entry in listing.iter() {
        let entry = entry?;
        let name = entry.file_name().to_bytes();
        if name != b"." && name != b".." {
            let name = OsStr::from_bytes(name).to_owned();
            let metadata = fstatat(directory, name.as_os_str(), AtFlags::AT_SYMLINK_NOFOLLOW)?;
            let directory = nix::sys::stat::SFlag::from_bits_truncate(metadata.st_mode)
                == nix::sys::stat::SFlag::S_IFDIR;
            children.push((!directory, name));
        }
    }
    children.sort_by_key(|(file, name)| (*file, name == super::FILE));
    for (_, name) in children {
        let metadata = fstatat(directory, name.as_os_str(), AtFlags::AT_SYMLINK_NOFOLLOW)?;
        if let Some(expected) = interrupted
            && name != super::FILE
        {
            if name != "data" {
                return Err(io::Error::other("unexpected interrupted eviction entry"));
            }
            let data = File::from(openat(
                directory,
                name.as_os_str(),
                OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
                Mode::empty(),
            )?);
            let found = data.metadata()?;
            if !same(&fstat(&data)?, &metadata) || (found.dev(), found.ino()) != expected {
                return Err(io::Error::other(
                    "retaining a replacement from an interrupted eviction",
                ));
            }
        }
        let kind = nix::sys::stat::SFlag::from_bits_truncate(metadata.st_mode);
        if kind == nix::sys::stat::SFlag::S_IFDIR {
            let child = File::from(openat(
                directory,
                name.as_os_str(),
                DIRECTORY,
                Mode::empty(),
            )?);
            if !same(&fstat(&child)?, &metadata) {
                return Err(io::Error::other("eviction child changed identity"));
            }
            clear(&child, false)?;
            if !same(
                &fstatat(directory, name.as_os_str(), AtFlags::AT_SYMLINK_NOFOLLOW)?,
                &metadata,
            ) {
                return Err(io::Error::other("eviction child changed identity"));
            }
            unlinkat(directory, name.as_os_str(), UnlinkatFlags::RemoveDir)?;
        } else {
            unlinkat(directory, name.as_os_str(), UnlinkatFlags::NoRemoveDir)?;
        }
    }
    Ok(())
}
