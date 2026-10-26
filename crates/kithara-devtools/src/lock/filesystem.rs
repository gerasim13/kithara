use std::{
    ffi::OsStr,
    fs::File,
    io,
    os::unix::fs::MetadataExt as _,
    path::{self, Component, Path},
};

use nix::{
    errno::Errno,
    fcntl::{OFlag, open, openat},
    sys::stat::Mode,
};

pub(crate) const DIRECTORY: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_DIRECTORY)
    .union(OFlag::O_NOFOLLOW)
    .union(OFlag::O_CLOEXEC);

pub(super) fn open_lock(path: &Path) -> io::Result<File> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("lock needs a parent"))?;
    let parent = open_directory(&path::absolute(parent)?).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("opening lock parent {}: {error}", parent.display()),
        )
    })?;
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("lock needs a name"))?;
    create_or_open_at(&parent, name)
}

pub(crate) fn create_or_open_at(parent: &File, name: &OsStr) -> io::Result<File> {
    let access = OFlag::O_RDWR | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC;
    let file = File::from(
        match openat(
            parent,
            name,
            access | OFlag::O_CREAT | OFlag::O_EXCL,
            Mode::S_IRUSR | Mode::S_IWUSR,
        ) {
            Ok(file) => file,
            Err(Errno::EEXIST) => openat(parent, name, access, Mode::empty())?,
            Err(error) => return Err(error.into()),
        },
    );
    unique_file(&file)?;
    Ok(file)
}

pub(crate) fn unique_file(file: &File) -> io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "writable entry must be a singly linked regular file",
        ));
    }
    Ok(())
}

pub(crate) fn open_directory(path: &Path) -> io::Result<File> {
    #[cfg(target_os = "macos")]
    let native = ["/tmp", "/var"].into_iter().find_map(|alias| {
        path.strip_prefix(alias).ok().map(|tail| {
            Path::new("/private")
                .join(alias.trim_start_matches('/'))
                .join(tail)
        })
    });
    #[cfg(target_os = "macos")]
    let path = native.as_deref().unwrap_or(path);
    let root = File::from(open(Path::new("/"), DIRECTORY, Mode::empty())?);
    descend(&root, path.strip_prefix("/").map_err(io::Error::other)?)
}

pub(crate) fn descend(root: &File, relative: &Path) -> io::Result<File> {
    let mut directory = root.try_clone()?;
    for part in relative.components() {
        let Component::Normal(name) = part else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "non-normal directory component",
            ));
        };
        directory = File::from(openat(&directory, name, DIRECTORY, Mode::empty())?);
    }
    Ok(directory)
}
