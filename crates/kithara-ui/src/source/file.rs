use std::{
    cell::RefCell,
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

use kithara_platform::sync::Arc;

use crate::{
    error::UiDocError,
    ids::SourceUri,
    source::{
        resolve_uri,
        uri::{LoadedBytes, LoadedSource, SourceResolver},
    },
};

/// Reads sources from one directory on disk.
///
/// The root is made real once, when the resolver is built, so every later read
/// compares against a path that already exists. A name reaching outside it is
/// refused on the same terms whether it spelled its way out with `..` or was
/// led out by a symlink.
///
/// What it has read, it keeps. A document graph names the same module from
/// several places, and parsing is already deduplicated by uri while reading is
/// not; without this the application would open a third of its files twice and
/// the gallery nearly two thirds. A name that was not there is not kept, so a
/// file appearing later is still found.
#[derive(Debug)]
pub struct FileResolver {
    blobs: RefCell<BTreeMap<String, Arc<[u8]>>>,
    root: PathBuf,
    texts: RefCell<BTreeMap<String, String>>,
}

impl FileResolver {
    /// Opens `root` as the directory every name is resolved against.
    ///
    /// # Errors
    /// Returns the io error when `root` cannot be made real.
    pub fn new<P: AsRef<Path>>(root: P) -> io::Result<Self> {
        Ok(Self {
            blobs: RefCell::default(),
            root: fs::canonicalize(root)?,
            texts: RefCell::default(),
        })
    }

    fn real(&self, origin: &SourceUri, uri: &SourceUri, rel: &str) -> Result<PathBuf, UiDocError> {
        let real = fs::canonicalize(self.root.join(&uri.0))
            .map_err(|error| refusal(origin.clone(), rel, &error))?;
        if real.starts_with(&self.root) {
            Ok(real)
        } else {
            Err(UiDocError::RootEscape {
                origin: origin.clone(),
                rel: rel.to_owned(),
            })
        }
    }
}

/// Tells a name that is not there from one that is there and would not open.
fn refusal(origin: SourceUri, rel: &str, error: &io::Error) -> UiDocError {
    if error.kind() == io::ErrorKind::NotFound {
        return UiDocError::NotFound {
            origin,
            rel: rel.to_owned(),
        };
    }
    UiDocError::Unreadable {
        origin,
        rel: rel.to_owned(),
        source: io::Error::new(error.kind(), error.to_string()),
    }
}

impl SourceResolver for FileResolver {
    fn load(&self, base: Option<&SourceUri>, rel: &str) -> Result<LoadedSource, UiDocError> {
        let uri = resolve_uri(base, rel)?;
        if let Some(text) = self.texts.borrow().get(&uri.0) {
            return Ok(LoadedSource {
                uri,
                text: text.clone(),
            });
        }
        let origin = base.cloned().unwrap_or_else(|| uri.clone());
        let path = self.real(&origin, &uri, rel)?;
        let text = fs::read_to_string(path).map_err(|error| refusal(origin, rel, &error))?;
        self.texts.borrow_mut().insert(uri.0.clone(), text.clone());
        Ok(LoadedSource { uri, text })
    }

    fn bytes(&self, base: Option<&SourceUri>, rel: &str) -> Result<LoadedBytes, UiDocError> {
        let uri = resolve_uri(base, rel)?;
        if let Some(bytes) = self.blobs.borrow().get(&uri.0) {
            return Ok(LoadedBytes {
                uri,
                bytes: Arc::clone(bytes),
            });
        }
        let origin = base.cloned().unwrap_or_else(|| uri.clone());
        let path = self.real(&origin, &uri, rel)?;
        let read = fs::read(path).map_err(|error| refusal(origin, rel, &error))?;
        let bytes = Arc::<[u8]>::from(read.as_slice());
        self.blobs
            .borrow_mut()
            .insert(uri.0.clone(), Arc::clone(&bytes));
        Ok(LoadedBytes { uri, bytes })
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;
    use tempfile::TempDir;

    use super::*;

    fn rooted() -> (TempDir, FileResolver) {
        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("modules/deck")).unwrap();
        fs::write(root.path().join("player.klayout.ron"), "(id: \"player\")").unwrap();
        fs::write(
            root.path().join("modules/deck/transport.kmodule.ron"),
            "(id: \"transport\")",
        )
        .unwrap();
        fs::write(root.path().join("sprites.bin"), [0xff, 0x00, 0xfe]).unwrap();
        let resolver = FileResolver::new(root.path()).unwrap();
        (root, resolver)
    }

    #[kithara::test]
    fn a_file_under_the_root_is_read() {
        let (_root, resolver) = rooted();

        let loaded = resolver.load(None, "player.klayout.ron").unwrap();

        assert_eq!(loaded.text, "(id: \"player\")");
    }

    #[kithara::test]
    fn a_loaded_file_reports_the_uri_it_was_asked_for() {
        let (_root, resolver) = rooted();

        let loaded = resolver.load(None, "player.klayout.ron").unwrap();

        assert_eq!(loaded.uri.0, "player.klayout.ron");
    }

    #[kithara::test]
    fn a_relative_include_resolves_against_the_base_dir() {
        let (_root, resolver) = rooted();
        let base = SourceUri("modules/deck.kmodule.ron".into());

        let loaded = resolver
            .load(Some(&base), "deck/transport.kmodule.ron")
            .unwrap();

        assert_eq!(loaded.uri.0, "modules/deck/transport.kmodule.ron");
    }

    #[kithara::test]
    fn a_name_the_root_does_not_hold_is_not_found() {
        let (_root, resolver) = rooted();

        let error = resolver.load(None, "nowhere.klayout.ron").unwrap_err();

        assert!(matches!(error, UiDocError::NotFound { .. }));
    }

    #[kithara::test]
    fn a_name_that_spells_its_way_out_is_refused() {
        let (_root, resolver) = rooted();
        let base = SourceUri("modules/deck.kmodule.ron".into());

        let error = resolver.load(Some(&base), "../../etc/passwd").unwrap_err();

        assert!(matches!(error, UiDocError::RootEscape { .. }));
    }

    /// A name may stay inside the root and still be led out of it. The root is
    /// real, so the file it opens has to be real under the same prefix.
    #[cfg(unix)]
    #[kithara::test]
    fn a_symlink_leading_out_of_the_root_is_refused() {
        let (root, resolver) = rooted();
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("secret"), "not yours").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret"),
            root.path().join("linked.ron"),
        )
        .unwrap();

        let error = resolver.load(None, "linked.ron").unwrap_err();

        assert!(matches!(error, UiDocError::RootEscape { .. }));
    }

    /// A directory is present, so it is not missing; it simply does not open as
    /// a document, and the two refusals must not read the same.
    #[kithara::test]
    fn a_directory_is_unreadable_rather_than_missing() {
        let (_root, resolver) = rooted();

        let error = resolver.load(None, "modules").unwrap_err();

        assert!(matches!(error, UiDocError::Unreadable { .. }));
    }

    #[kithara::test]
    fn bytes_read_a_source_that_is_not_utf8() {
        let (_root, resolver) = rooted();

        let loaded = resolver.bytes(None, "sprites.bin").unwrap();

        assert_eq!(&*loaded.bytes, &[0xff, 0x00, 0xfe]);
    }

    /// Deleting the file between the two reads is what proves the second one
    /// never reached the filesystem: nothing on disk could have answered it.
    #[kithara::test]
    fn a_source_read_once_is_answered_again_without_the_file() {
        let (root, resolver) = rooted();
        let first = resolver.load(None, "player.klayout.ron").unwrap();
        fs::remove_file(root.path().join("player.klayout.ron")).unwrap();

        let again = resolver.load(None, "player.klayout.ron").unwrap();

        assert_eq!(again.text, first.text);
    }

    #[kithara::test]
    fn bytes_read_once_are_answered_again_without_the_file() {
        let (root, resolver) = rooted();
        let first = resolver.bytes(None, "sprites.bin").unwrap();
        fs::remove_file(root.path().join("sprites.bin")).unwrap();

        let again = resolver.bytes(None, "sprites.bin").unwrap();

        assert_eq!(&*again.bytes, &*first.bytes);
    }

    /// One kept name must not answer for another, so the second read has to be
    /// the second file and not the first one handed back.
    #[kithara::test]
    fn a_kept_source_does_not_answer_for_another_name() {
        let (_root, resolver) = rooted();
        resolver.load(None, "player.klayout.ron").unwrap();
        let base = SourceUri("modules/deck.kmodule.ron".into());

        let other = resolver
            .load(Some(&base), "deck/transport.kmodule.ron")
            .unwrap();

        assert_eq!(other.text, "(id: \"transport\")");
    }

    /// The two doors keep two sets. A name read as text is not answered as
    /// bytes from what was kept, or a picture could arrive as a document.
    #[kithara::test]
    fn a_kept_text_is_not_answered_as_bytes() {
        let (root, resolver) = rooted();
        resolver.load(None, "player.klayout.ron").unwrap();
        fs::remove_file(root.path().join("player.klayout.ron")).unwrap();

        let error = resolver.bytes(None, "player.klayout.ron").unwrap_err();

        assert!(matches!(error, UiDocError::NotFound { .. }));
    }

    /// A name that was not there is not kept, so a package repaired while the
    /// resolver lives is still found.
    #[kithara::test]
    fn a_name_that_was_missing_is_found_once_it_appears() {
        let (root, resolver) = rooted();
        resolver.load(None, "later.klayout.ron").unwrap_err();
        fs::write(root.path().join("later.klayout.ron"), "(id: \"later\")").unwrap();

        let loaded = resolver.load(None, "later.klayout.ron").unwrap();

        assert_eq!(loaded.text, "(id: \"later\")");
    }

    #[kithara::test]
    fn bytes_refuse_a_name_the_root_does_not_hold() {
        let (_root, resolver) = rooted();

        let error = resolver.bytes(None, "sprites/none.png").unwrap_err();

        assert!(matches!(error, UiDocError::NotFound { .. }));
    }
}
