use std::{path::Path, sync::OnceLock};

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
    /// Asset read from the store on first use.
    #[must_use]
    pub const fn on_disk(entry: &'static AssetEntry, cell: &'static OnceLock<Vec<u8>>) -> Self {
        Self {
            entry,
            source: Source::OnDisk(cell),
        }
    }

    /// Asset baked into the binary at compile time.
    #[must_use]
    pub const fn embedded(entry: &'static AssetEntry, bytes: &'static [u8]) -> Self {
        Self {
            entry,
            source: Source::Embedded(bytes),
        }
    }

    /// Bytes of the asset.
    ///
    /// # Panics
    ///
    /// Panics when the store entry is missing. That means the build script did
    /// not run for this build: run `cargo build -p kithara-test-fixtures`.
    #[must_use]
    pub fn bytes(&self) -> &'static [u8] {
        match self.source {
            Source::Embedded(bytes) => bytes,
            Source::OnDisk(cell) => cell.get_or_init(|| {
                std::fs::read(self.entry.path).unwrap_or_else(|error| {
                    panic!(
                        "kithara-test-fixtures: asset `{}` is missing from the store at {} ({error}); \
                         run `cargo build -p kithara-test-fixtures` to materialize it",
                        self.entry.name, self.entry.path,
                    )
                })
            }),
        }
    }

    /// Manifest record this handle points at: name, id, path, content type.
    #[must_use]
    pub const fn entry(&self) -> &'static AssetEntry {
        self.entry
    }

    /// Store path, or `None` for an asset baked into the binary.
    #[must_use]
    pub fn path(&self) -> Option<&'static Path> {
        match self.source {
            Source::Embedded(_) => None,
            Source::OnDisk(_) => Some(Path::new(self.entry.path)),
        }
    }
}
