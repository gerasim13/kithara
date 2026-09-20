use std::{fs, path::PathBuf};

use objc2_foundation::{
    NSFileManager, NSNumber, NSSearchPathDirectory, NSSearchPathDomainMask, NSString,
    NSURLIsExcludedFromBackupKey,
};

/// Creates `Documents/Files/Kithara` and excludes it from backups.
pub fn prepare_playback_cache_directory() -> Option<PathBuf> {
    let documents = NSFileManager::defaultManager()
        .URLForDirectory_inDomain_appropriateForURL_create_error(
            NSSearchPathDirectory::DocumentDirectory,
            NSSearchPathDomainMask::UserDomainMask,
            None,
            true,
        )
        .ok()?;
    let relative_path = NSString::from_str("Files/Kithara");
    let directory = documents.URLByAppendingPathComponent_isDirectory(&relative_path, true)?;
    let path = PathBuf::from(directory.path()?.to_string());
    fs::create_dir_all(&path).ok()?;

    let excluded = NSNumber::new_bool(true);
    // SAFETY: NSURLIsExcludedFromBackupKey accepts an NSNumber boolean.
    let _ = unsafe {
        directory.setResourceValue_forKey_error(Some(&excluded), NSURLIsExcludedFromBackupKey)
    };

    Some(path)
}
