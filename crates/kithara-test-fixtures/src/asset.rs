use std::{path::Path, sync::OnceLock};

use thiserror::Error;

/// One generated asset as the manifest records it.
#[derive(Debug)]
#[non_exhaustive]
pub struct AssetEntry {
    /// MIME type declared by the generator.
    pub content_type: &'static str,
    /// Content address inside the build fingerprint's namespace.
    pub id: &'static str,
    /// Accessor name, `{func}_{case}`.
    pub name: &'static str,
    /// Absolute path in the store.
    pub path: &'static str,
    /// Redacted build-time reason an optional asset is unavailable.
    pub unavailable: Option<&'static str>,
}

/// Failure to access one generated or hydrated fixture.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum AssetError {
    /// An optional producer did not materialize this asset.
    #[error("fixture `{name}` is unavailable: {reason}")]
    Unavailable {
        /// Generated accessor name.
        name: &'static str,
        /// Redacted build-time failure.
        reason: &'static str,
    },
    /// A required on-disk entry disappeared after the build.
    #[error("fixture `{name}` is missing from {path}: {source}")]
    Read {
        /// Generated accessor name.
        name: &'static str,
        /// Expected store path.
        path: &'static str,
        /// Filesystem failure.
        #[source]
        source: std::io::Error,
    },
}

/// Handle to one generated asset.
pub struct Asset {
    entry: &'static AssetEntry,
    source: Source,
}

enum Source {
    Embedded(&'static [u8]),
    OnDisk(&'static OnceLock<Vec<u8>>),
}

impl Asset {
    /// Bytes of the asset.
    ///
    /// # Panics
    ///
    /// Panics when an optional asset is unavailable or a store entry is
    /// missing. A missing required entry means the build script did not run for
    /// this build: run `cargo build -p kithara-test-fixtures`.
    #[must_use]
    pub fn bytes(&self) -> &'static [u8] {
        self.try_bytes().unwrap_or_else(|error| panic!("{error}"))
    }

    /// Asset baked into the binary at compile time.
    #[must_use]
    pub const fn embedded(entry: &'static AssetEntry, bytes: &'static [u8]) -> Self {
        Self {
            entry,
            source: Source::Embedded(bytes),
        }
    }

    /// Manifest record this handle points at: name, id, path, content type.
    #[must_use]
    pub const fn entry(&self) -> &'static AssetEntry {
        self.entry
    }

    /// Asset read from the store on first use.
    #[must_use]
    pub const fn on_disk(entry: &'static AssetEntry, cell: &'static OnceLock<Vec<u8>>) -> Self {
        Self {
            entry,
            source: Source::OnDisk(cell),
        }
    }

    /// Store path, or `None` for an asset baked into the binary.
    #[must_use]
    pub fn path(&self) -> Option<&'static Path> {
        match self.source {
            Source::Embedded(_) => None,
            Source::OnDisk(_) => Some(Path::new(self.entry.path)),
        }
    }

    /// Bytes of the asset, or the redacted reason an optional producer failed.
    ///
    /// # Errors
    ///
    /// Returns [`AssetError::Unavailable`] for an optional asset that was not
    /// hydrated and [`AssetError::Read`] when an on-disk entry disappeared.
    pub fn try_bytes(&self) -> Result<&'static [u8], AssetError> {
        if let Some(reason) = self.entry.unavailable {
            return Err(AssetError::Unavailable {
                reason,
                name: self.entry.name,
            });
        }
        match self.source {
            Source::Embedded(bytes) => Ok(bytes),
            Source::OnDisk(cell) => {
                if let Some(bytes) = cell.get() {
                    return Ok(bytes);
                }
                let loaded = std::fs::read(self.entry.path).map_err(|source| AssetError::Read {
                    source,
                    name: self.entry.name,
                    path: self.entry.path,
                })?;
                Ok(cell.get_or_init(|| loaded))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::{Asset, AssetEntry, AssetError, OnceLock};

    #[kithara::test(native, flash(false))]
    fn unavailable_asset_returns_the_build_failure() {
        static ENTRY: AssetEntry = AssetEntry {
            name: "remote_reference",
            id: "remote-id",
            path: "/missing/remote.m3u8",
            content_type: "application/vnd.apple.mpegurl",
            unavailable: Some("HTTP 403; refresh KITHARA_TOKEN"),
        };
        static BYTES: OnceLock<Vec<u8>> = OnceLock::new();
        let asset = Asset::on_disk(&ENTRY, &BYTES);

        assert!(matches!(
            asset.try_bytes(),
            Err(AssetError::Unavailable {
                name: "remote_reference",
                reason: "HTTP 403; refresh KITHARA_TOKEN",
            })
        ));
    }
}
