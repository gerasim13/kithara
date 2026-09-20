use std::{fs, path::PathBuf};

use objc2_foundation::{
    NSFileManager, NSNumber, NSSearchPathDirectory, NSSearchPathDomainMask, NSString,
    NSURLIsExcludedFromBackupKey,
};

/// Creates `relative_path` below Documents and excludes it from backups.
pub fn prepare_documents_directory(relative_path: &str) -> Option<PathBuf> {
    let documents = NSFileManager::defaultManager()
        .URLForDirectory_inDomain_appropriateForURL_create_error(
            NSSearchPathDirectory::DocumentDirectory,
            NSSearchPathDomainMask::UserDomainMask,
            None,
            true,
        )
        .ok()?;
    let relative_path = NSString::from_str(relative_path);
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
