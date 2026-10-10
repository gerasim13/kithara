use std::{
    any::Any,
    collections::{BTreeMap, HashMap},
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use keyring_core::{
    Entry, Error, Result,
    api::{Credential, CredentialApi, CredentialStoreApi},
};
use kithara_platform::sync::Arc;
use serde_yaml_ng::{Mapping, Value};
use tempfile::NamedTempFile;

use super::Secrets;

type Values = BTreeMap<String, String>;

/// The application's secrets as plain text in the [`Secrets::SECTION`] of a
/// configuration overlay.
#[derive(Clone)]
pub(super) struct Store {
    path: PathBuf,
}

impl Store {
    /// The store in the overlay at `path`.
    pub(super) fn new(path: &Path) -> Arc<Self> {
        Arc::new(Self {
            path: path.to_owned(),
        })
    }

    /// The overlay and the secrets it holds; a missing or empty file holds
    /// none.
    fn read(&self) -> Result<(Mapping, Values)> {
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(platform(error)),
        };
        let document = match serde_yaml_ng::from_str(&text).map_err(|_| self.malformed())? {
            Value::Null => Mapping::new(),
            Value::Mapping(document) => document,
            _ => return Err(self.malformed()),
        };
        let values = document
            .get(Secrets::SECTION)
            .cloned()
            .map_or(Ok(Values::new()), serde_yaml_ng::from_value)
            .map_err(|_| self.malformed())?;
        Ok((document, values))
    }

    fn write(&self, mut document: Mapping, values: &Values) -> Result<()> {
        let section = serde_yaml_ng::to_value(values)
            .map_err(|error| Error::PlatformFailure(error.into()))?;
        document.insert(Value::from(Secrets::SECTION), section);
        let text = serde_yaml_ng::to_string(&document)
            .map_err(|error| Error::PlatformFailure(error.into()))?;
        self.replace(text.as_bytes()).map_err(platform)
    }

    fn replace(&self, bytes: &[u8]) -> io::Result<()> {
        let dir = self.path.parent().ok_or(io::ErrorKind::InvalidInput)?;
        let mut file = NamedTempFile::new_in(dir)?;
        file.write_all(bytes)?;
        file.as_file().sync_all()?;
        file.persist(&self.path)?;
        Ok(())
    }

    fn malformed(&self) -> Error {
        Error::BadStoreFormat(self.path.display().to_string())
    }
}

impl CredentialStoreApi for Store {
    fn vendor(&self) -> String {
        String::from("kithara overlay")
    }

    fn id(&self) -> String {
        self.path.display().to_string()
    }

    fn build(
        &self,
        _service: &str,
        user: &str,
        _modifiers: Option<&HashMap<&str, &str>>,
    ) -> Result<Entry> {
        Ok(Entry::new_with_credential(Arc::new(Secret {
            store: self.clone(),
            key: user.to_owned(),
        })))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The value under one key of a [`Store`].
struct Secret {
    store: Store,
    key: String,
}

impl CredentialApi for Secret {
    fn set_secret(&self, secret: &[u8]) -> Result<()> {
        let value = String::from_utf8(secret.to_vec())
            .map_err(|error| Error::BadEncoding(error.into_bytes()))?;
        let (document, mut values) = self.store.read()?;
        values.insert(self.key.clone(), value);
        self.store.write(document, &values)
    }

    fn get_secret(&self) -> Result<Vec<u8>> {
        let (_, mut values) = self.store.read()?;
        values
            .remove(&self.key)
            .map(String::into_bytes)
            .ok_or(Error::NoEntry)
    }

    fn delete_credential(&self) -> Result<()> {
        let (document, mut values) = self.store.read()?;
        values.remove(&self.key).ok_or(Error::NoEntry)?;
        self.store.write(document, &values)
    }

    fn get_credential(&self) -> Result<Option<Arc<Credential>>> {
        self.get_secret().map(|_| None)
    }

    fn get_specifiers(&self) -> Option<(String, String)> {
        None
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn platform(error: io::Error) -> Error {
    if error.kind() == io::ErrorKind::PermissionDenied {
        Error::NoStorageAccess(error.into())
    } else {
        Error::PlatformFailure(error.into())
    }
}

#[cfg(test)]
mod tests {
    use std::error;

    use kithara_test_utils::kithara;
    use tempfile::TempDir;

    use super::*;
    use crate::SecretError;

    fn secrets(path: &Path) -> Secrets {
        Secrets::new(Ok(Store::new(path)))
    }

    /// A value set through one store reads back through a new one until
    /// deleted, beside the overlay's other keys; a file the store creates
    /// only its owner may read.
    #[kithara::test]
    fn a_stored_value_round_trips_through_the_overlay_and_keeps_its_other_keys() {
        let dir = TempDir::new().expect("a temp dir");
        let created = dir.path().join("created.yaml");
        let overlay = dir.path().join("overlay.yaml");
        fs::write(
            &overlay,
            "hls:\n  size_probe_method: head\nsecrets:\n  other: kept\n",
        )
        .expect("the overlay is written");

        assert_eq!(
            secrets(&created).get("probe").expect("the store reads"),
            None
        );
        secrets(&created)
            .delete("probe")
            .expect("a missing value is already deleted");
        assert!(!created.exists());
        secrets(&created)
            .set("probe", "value")
            .expect("the store creates the file");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mode = fs::metadata(&created)
                .expect("the file exists")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        secrets(&overlay)
            .set("probe", "value")
            .expect("the store writes");
        assert_eq!(
            secrets(&overlay)
                .get("probe")
                .expect("the store reads")
                .as_deref(),
            Some("value")
        );
        secrets(&overlay)
            .delete("probe")
            .expect("the store deletes");

        assert_eq!(
            secrets(&overlay).get("probe").expect("the store reads"),
            None
        );
        assert_eq!(
            secrets(&overlay)
                .get("other")
                .expect("the store reads")
                .as_deref(),
            Some("kept")
        );
        let kept: Value =
            serde_yaml_ng::from_str(&fs::read_to_string(&overlay).expect("the overlay reads"))
                .expect("the overlay parses");
        assert_eq!(kept["hls"]["size_probe_method"].as_str(), Some("head"));
    }

    #[kithara::test]
    fn a_corrupt_file_fails_the_call_and_keeps_its_contents_out_of_the_error() {
        let dir = TempDir::new().expect("a temp dir");
        let path = dir.path().join("kithara.yaml");
        fs::write(&path, "eyJ-leaked-token").expect("the file is written");

        let error = secrets(&path)
            .get("probe")
            .expect_err("the file is not a map");

        assert!(matches!(error, SecretError::Platform(_)));
        let chain: String =
            std::iter::successors(Some(&error as &dyn error::Error), |error| error.source())
                .map(|error| format!("{error} {error:?} "))
                .collect();
        assert!(!chain.contains("eyJ-leaked-token"), "{chain}");
        assert!(matches!(
            secrets(&path).set("probe", "value"),
            Err(SecretError::Platform(_))
        ));
        assert!(matches!(
            secrets(&path).delete("probe"),
            Err(SecretError::Platform(_))
        ));
        assert_eq!(
            fs::read_to_string(&path).expect("the file reads"),
            "eyJ-leaked-token"
        );
    }
}
