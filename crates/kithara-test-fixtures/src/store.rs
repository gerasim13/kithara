use std::{
    fs::{self, File, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

/// Overrides the store root. The build fingerprint is always appended to it.
pub const STORE_ENV: &str = "KITHARA_FIXTURE_CACHE";

/// Leading digest bytes kept in an asset id. 128 bits: short enough to read in
/// a path, wide enough that the build script's collision check never fires.
const ASSET_ID_BYTES: usize = 16;

/// Hex length of a build fingerprint (`u64`), shared by the build script that
/// produces it and the contract test that checks the namespace layout.
pub const FINGERPRINT_HEX_LEN: usize = 16;

/// Stable identity of one asset case: `sha2-256(func || 0x00 || case)`.
///
/// The pair is unique by construction — every accessor lands in one flat
/// module, so two cases sharing both halves could not coexist there.
#[must_use]
pub fn asset_id(func: &str, case: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(func.as_bytes());
    hasher.update([0u8]);
    hasher.update(case.as_bytes());
    hex::encode(&hasher.finalize()[..ASSET_ID_BYTES])
}

/// Store root when [`STORE_ENV`] is unset.
#[must_use]
pub fn default_root() -> PathBuf {
    std::env::temp_dir().join("kithara-fixture-cache")
}

/// Store root: [`STORE_ENV`] when set, [`default_root`] otherwise.
#[must_use]
pub fn root_from_env() -> PathBuf {
    std::env::var_os(STORE_ENV).map_or_else(default_root, PathBuf::from)
}

/// Directory holding every entry of one build fingerprint.
///
/// The fingerprint is the only thing between a changed generator and the bytes
/// the previous one produced: entries are content-addressed over the accessor
/// name alone.
#[must_use]
pub fn namespace(root: &Path, fingerprint: &str) -> PathBuf {
    root.join(fingerprint)
}

/// Path of one single-file entry inside a namespace.
#[must_use]
pub fn entry_path(namespace: &Path, id: &str, ext: &str) -> PathBuf {
    namespace.join(format!("{id}.{ext}"))
}

/// Reads one entry. An empty file counts as absent — a half-written entry must
/// never be served.
#[must_use]
pub fn read_entry(namespace: &Path, id: &str, ext: &str) -> Option<Vec<u8>> {
    let bytes = fs::read(entry_path(namespace, id, ext)).ok()?;
    if bytes.is_empty() { None } else { Some(bytes) }
}

/// Writes one entry atomically: temporary file, `sync_all`, rename.
///
/// # Errors
///
/// Returns the underlying error when the namespace cannot be created or the
/// bytes cannot be written, synced, or renamed into place.
pub fn write_entry(
    namespace: &Path,
    id: &str,
    ext: &str,
    bytes: &[u8],
) -> std::io::Result<PathBuf> {
    fs::create_dir_all(namespace)?;
    let tmp_path = namespace.join(format!("{id}.{ext}.tmp.{}", std::process::id()));
    let write = (|| -> std::io::Result<()> {
        let mut file = File::create(&tmp_path)?;
        file.write_all(bytes)?;
        file.sync_all()
    })();
    if let Err(error) = write {
        // The write already failed; a failure to clean up must not replace it.
        drop(fs::remove_file(&tmp_path));
        return Err(error);
    }
    let final_path = entry_path(namespace, id, ext);
    fs::rename(&tmp_path, &final_path)?;
    Ok(final_path)
}

/// Held while one entry is produced; serializes producers across processes.
#[must_use = "the lock must be held while the entry is produced"]
pub struct EntryLock {
    _file: File,
}

/// Takes the exclusive lock for one entry, creating the namespace if needed.
///
/// Callers must re-check the entry after acquiring the lock: a second producer
/// may have finished it while this one waited.
///
/// # Errors
///
/// Returns the underlying error when the namespace or the lock file cannot be
/// created, or when the lock cannot be taken.
pub fn lock_entry(namespace: &Path, id: &str) -> std::io::Result<EntryLock> {
    fs::create_dir_all(namespace)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(namespace.join(format!("{id}.lock")))?;
    file.lock()?;
    Ok(EntryLock { _file: file })
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;
    use tempfile::TempDir;

    use super::*;

    #[kithara::test(native, flash(false))]
    fn id_is_stable_and_separates_case_from_function() {
        let a = asset_id("sine_wav", "a440_6s");
        let same = asset_id("sine_wav", "a440_6s");
        let other_case = asset_id("sine_wav", "a440_2s");
        let other_func = asset_id("sine_mp3", "a440_6s");
        let swapped = asset_id("a440_6s", "sine_wav");

        assert_eq!(a, same);
        assert_ne!(a, other_case);
        assert_ne!(a, other_func);
        assert_ne!(a, swapped, "the separator must keep the halves apart");
        assert_eq!(a.len(), ASSET_ID_BYTES * 2);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[kithara::test(native, flash(false))]
    fn entry_path_lives_under_the_fingerprint_namespace() {
        let root = Path::new("root");
        let namespace = namespace(root, "0123456789abcdef");
        let path = entry_path(&namespace, "deadbeef", "wav");

        assert!(path.starts_with(root.join("0123456789abcdef")));
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("deadbeef.wav"),
        );
    }

    #[kithara::test(native, flash(false))]
    fn roundtrip_write_then_read() {
        let dir = TempDir::new().expect("temp dir");
        let namespace = dir.path().join("fingerprint");

        assert!(read_entry(&namespace, "id", "wav").is_none());
        let written = write_entry(&namespace, "id", "wav", b"payload").expect("write entry");
        assert_eq!(written, entry_path(&namespace, "id", "wav"));
        assert_eq!(
            read_entry(&namespace, "id", "wav").as_deref(),
            Some(b"payload".as_slice()),
        );
    }

    #[kithara::test(native, flash(false))]
    fn empty_entry_is_a_miss() {
        let dir = TempDir::new().expect("temp dir");
        let namespace = dir.path().join("fingerprint");
        write_entry(&namespace, "id", "wav", b"").expect("write empty entry");

        assert!(read_entry(&namespace, "id", "wav").is_none());
    }

    #[kithara::test(native, flash(false))]
    fn write_leaves_no_temporary_behind() {
        let dir = TempDir::new().expect("temp dir");
        let namespace = dir.path().join("fingerprint");
        write_entry(&namespace, "id", "wav", b"payload").expect("write entry");

        let leftovers: Vec<_> = fs::read_dir(&namespace)
            .expect("read namespace")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".tmp."))
            .collect();

        assert!(leftovers.is_empty(), "temporary files left: {leftovers:?}");
    }

    #[kithara::test(native, flash(false))]
    fn entry_lock_is_exclusive_and_released_on_drop() {
        let dir = TempDir::new().expect("temp dir");
        let namespace = dir.path().join("fingerprint");
        let held = lock_entry(&namespace, "id").expect("take entry lock");
        let contender = OpenOptions::new()
            .read(true)
            .write(true)
            .open(namespace.join("id.lock"))
            .expect("open the lock file a second time");

        assert!(
            matches!(contender.try_lock(), Err(fs::TryLockError::WouldBlock)),
            "the entry lock must exclude a second producer",
        );

        drop(held);
        contender
            .try_lock()
            .expect("the entry lock must release with its file handle");
    }

    #[kithara::test(native, flash(false))]
    fn explicit_root_wins_over_the_default() {
        // Read-only probe: assert the default only when nothing overrides it,
        // because the `cold` profile exports the variable.
        if std::env::var_os(STORE_ENV).is_some() {
            return;
        }
        assert_eq!(root_from_env(), default_root());
        assert!(default_root().starts_with(std::env::temp_dir()));
    }
}
