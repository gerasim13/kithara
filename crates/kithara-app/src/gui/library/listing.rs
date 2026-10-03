use std::{
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
};

use kithara::stream::{AudioCodec, ContainerFormat};
use tracing::debug;

use super::track::Track;

/// What listing a folder found.
pub(super) enum Listing {
    Listed(Folder),
    /// The folder could not be read.
    Failed,
}

/// What a folder holds that the library shows.
pub(super) struct Folder {
    /// Its subfolders, by name.
    pub(super) folders: Vec<PathBuf>,
    /// Its files the app can play, by name.
    pub(super) tracks: Vec<Track>,
}

pub(super) fn list(folder: &Path) -> Listing {
    match read(folder) {
        Ok(listed) => Listing::Listed(listed),
        Err(error) => {
            debug!(?folder, %error, "the library cannot read a folder");
            Listing::Failed
        }
    }
}

fn read(folder: &Path) -> io::Result<Folder> {
    let mut files: Vec<PathBuf> = Vec::new();
    let mut folders: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(folder)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(OsStr::to_str) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let Ok(metadata) = fs::metadata(&path) else {
            continue;
        };
        if metadata.is_dir() {
            folders.push(path);
        } else if metadata.is_file() && playable(&path) {
            files.push(path);
        }
    }
    files.sort_unstable();
    folders.sort_unstable();
    Ok(Folder {
        folders,
        tracks: files
            .iter()
            .map(PathBuf::as_path)
            .filter_map(track)
            .collect(),
    })
}

fn track(file: &Path) -> Option<Track> {
    let title = file.file_stem()?.to_str()?.to_owned();
    Some(Track::new(title, file.to_str()?.to_owned()))
}

fn playable(path: &Path) -> bool {
    path.extension().and_then(OsStr::to_str).is_some_and(|ext| {
        AudioCodec::parse_extension(ext).is_some()
            || ContainerFormat::parse_extension(ext).is_some()
    })
}
