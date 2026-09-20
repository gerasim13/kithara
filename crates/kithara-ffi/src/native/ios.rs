use std::{fs, path::PathBuf};

use kithara::assets::StorageBackend;
use objc2_foundation::{
    NSFileManager, NSNumber, NSSearchPathDirectory, NSSearchPathDomainMask, NSString,
    NSURLIsExcludedFromBackupKey,
};
use tracing::warn;

pub(super) fn default_backend() -> StorageBackend {
    default_root().map_or_else(StorageBackend::default, |root| StorageBackend::Disk {
        root: PathBuf::from(root),
    })
}

fn default_root() -> Option<String> {
    let documents = NSFileManager::defaultManager()
        .URLForDirectory_inDomain_appropriateForURL_create_error(
            NSSearchPathDirectory::DocumentDirectory,
            NSSearchPathDomainMask::UserDomainMask,
            None,
            true,
        )
        .inspect_err(|error| warn!(%error, "could not resolve iOS Documents directory"))
        .ok()?;
    let cache = documents
        .URLByAppendingPathComponent_isDirectory(&NSString::from_str("Files/Kithara"), true)?;
    let root = cache.path()?.to_string();
    fs::create_dir_all(&root)
        .inspect_err(|error| warn!(%error, "could not prepare iOS playback cache"))
        .ok()?;
    let excluded = NSNumber::new_bool(true);
    // SAFETY: NSURLIsExcludedFromBackupKey accepts an NSNumber boolean.
    let backup = unsafe {
        cache.setResourceValue_forKey_error(Some(&excluded), NSURLIsExcludedFromBackupKey)
    };
    if let Err(error) = backup {
        warn!(%error, "could not exclude iOS playback cache from backup");
    }
    Some(root)
}
