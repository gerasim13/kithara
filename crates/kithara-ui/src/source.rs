use std::collections::{BTreeMap, BTreeSet};

use bon::Builder;
use kithara_platform::sync::Arc;

use crate::{error::UiDocError, ids::SourceUri};

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct LoadedSource {
    pub uri: SourceUri,
    pub text: String,
}

/// A source that is not text: the bytes, and where they came from.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct LoadedBytes {
    pub uri: SourceUri,
    pub bytes: Arc<[u8]>,
}

#[derive(Builder, Clone, Debug)]
#[non_exhaustive]
pub struct Limits {
    #[builder(default = 256 * 1024)]
    pub max_bytes: usize,
    #[builder(default = 8)]
    pub max_depth: usize,
    #[builder(default = 10_000)]
    pub max_nodes: usize,
}

/// Memory retained by the draw pools between frames.
#[derive(Builder, Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct DrawPoolLimits {
    /// Maximum reusable buffers kept by each pool. Zero is treated as one.
    #[builder(default = 64)]
    pub max_buffers: usize,
    /// Command slots retained by one returned draw-list buffer.
    #[builder(default = 512)]
    pub command_capacity: usize,
    /// Vector verbs retained by one returned path buffer.
    #[builder(default = 128)]
    pub path_capacity: usize,
    /// UTF-8 bytes retained by one returned text buffer.
    #[builder(default = 128)]
    pub text_capacity: usize,
}

impl Default for DrawPoolLimits {
    fn default() -> Self {
        Self::builder().build()
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::builder().build()
    }
}

/// Canonical compile configuration and its resource limits.
#[derive(Builder, Clone, Debug)]
#[builder(state_mod(vis = "pub"))]
#[non_exhaustive]
pub struct UiConfig {
    /// The extension kinds the application registers with its hosts.
    ///
    /// A document naming a `Custom` kind absent from this set is refused while
    /// it compiles, so no host is ever handed an extension it cannot mount.
    #[builder(default)]
    pub custom_kinds: BTreeSet<String>,
    #[builder(default)]
    pub limits: Limits,
    #[builder(default = 64 * 1024)]
    pub max_arena_bytes: usize,
    #[builder(default)]
    pub draw_pools: DrawPoolLimits,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self::builder().build()
    }
}

pub trait SourceResolver {
    /// Loads `rel`, resolved against the directory containing `base`.
    ///
    /// # Errors
    /// Returns [`UiDocError`] when the path escapes the root or is unavailable.
    fn load(&self, base: Option<&SourceUri>, rel: &str) -> Result<LoadedSource, UiDocError>;

    /// Loads `rel` as bytes, resolved against `base` on the same terms.
    ///
    /// A picture is not a document: a skin that names one reads it through
    /// this door rather than the one every text source comes through, because
    /// PNG bytes are not valid UTF-8 and would be refused on the way in.
    ///
    /// # Errors
    /// Returns [`UiDocError`] when the path escapes the root or is unavailable.
    fn bytes(&self, base: Option<&SourceUri>, rel: &str) -> Result<LoadedBytes, UiDocError>;
}

pub(crate) fn base_dir(base: Option<&SourceUri>) -> &str {
    let Some(base) = base else {
        return "";
    };
    base.0.rfind('/').map_or("", |index| &base.0[..index])
}

pub(crate) fn join_rel(dir: &str, rel: &str) -> Option<String> {
    let mut parts: Vec<&str> = if dir.is_empty() {
        Vec::new()
    } else {
        dir.split('/').collect()
    };
    for segment in rel.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    Some(parts.join("/"))
}

fn resolve_uri(base: Option<&SourceUri>, rel: &str) -> Result<SourceUri, UiDocError> {
    let origin = base.cloned().unwrap_or_else(|| SourceUri("<entry>".into()));
    if rel.starts_with('/') {
        return Err(UiDocError::RootEscape {
            origin,
            rel: rel.to_owned(),
        });
    }
    join_rel(base_dir(base), rel)
        .map(SourceUri)
        .ok_or_else(|| UiDocError::RootEscape {
            origin,
            rel: rel.to_owned(),
        })
}

#[derive(Debug, Default)]
pub struct MemResolver {
    blobs: BTreeMap<String, Arc<[u8]>>,
    files: BTreeMap<String, String>,
}

impl MemResolver {
    pub fn insert(&mut self, path: &str, text: &str) {
        self.files.insert(path.to_owned(), text.to_owned());
    }

    /// Adds a source that is not text, such as a picture a skin names.
    pub fn insert_bytes(&mut self, path: &str, bytes: &[u8]) {
        self.blobs.insert(path.to_owned(), Arc::from(bytes));
    }
}

impl SourceResolver for MemResolver {
    fn load(&self, base: Option<&SourceUri>, rel: &str) -> Result<LoadedSource, UiDocError> {
        let uri = resolve_uri(base, rel)?;
        let origin = base.cloned().unwrap_or_else(|| uri.clone());
        self.files
            .get(&uri.0)
            .map(|text| LoadedSource {
                uri,
                text: text.clone(),
            })
            .ok_or_else(|| UiDocError::NotFound {
                origin,
                rel: rel.to_owned(),
            })
    }

    fn bytes(&self, base: Option<&SourceUri>, rel: &str) -> Result<LoadedBytes, UiDocError> {
        let uri = resolve_uri(base, rel)?;
        let origin = base.cloned().unwrap_or_else(|| uri.clone());
        self.blobs
            .get(&uri.0)
            .map(|bytes| LoadedBytes {
                uri,
                bytes: Arc::clone(bytes),
            })
            .ok_or_else(|| UiDocError::NotFound {
                origin,
                rel: rel.to_owned(),
            })
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test]
    fn relative_include_resolves_against_base_dir() {
        let mut resolver = MemResolver::default();
        resolver.insert("modules/deck/transport.kmodule.ron", "x");
        let base = SourceUri("modules/deck.kmodule.ron".into());
        let loaded = resolver
            .load(Some(&base), "deck/transport.kmodule.ron")
            .unwrap();
        assert_eq!(loaded.uri.0, "modules/deck/transport.kmodule.ron");
    }

    #[kithara::test]
    fn parent_escape_is_rejected() {
        let resolver = MemResolver::default();
        let base = SourceUri("modules/deck.kmodule.ron".into());
        let error = resolver.load(Some(&base), "../../etc/passwd").unwrap_err();
        assert!(matches!(error, UiDocError::RootEscape { .. }));
    }

    #[kithara::test]
    fn absolute_path_is_rejected() {
        let resolver = MemResolver::default();
        let error = resolver.load(None, "/etc/passwd").unwrap_err();
        assert!(matches!(error, UiDocError::RootEscape { .. }));
    }

    #[kithara::test]
    fn bytes_resolve_against_the_base_dir_like_text_does() {
        let mut resolver = MemResolver::default();
        resolver.insert_bytes("skins/sprites/spinner.png", &[1, 2, 3]);
        let base = SourceUri("skins/kithara-dark.kskin.ron".into());
        let loaded = resolver.bytes(Some(&base), "sprites/spinner.png").unwrap();

        assert_eq!(loaded.uri.0, "skins/sprites/spinner.png");
    }

    /// The two doors hold two sets: a name answered as text is not answered as
    /// bytes, so a picture cannot be read as a document or the other way round.
    #[kithara::test]
    fn a_text_source_is_not_answered_as_bytes() {
        let mut resolver = MemResolver::default();
        resolver.insert("kithara-dark.kskin.ron", "(id: \"dark\")");
        let error = resolver.bytes(None, "kithara-dark.kskin.ron").unwrap_err();

        assert!(matches!(error, UiDocError::NotFound { .. }));
    }

    #[kithara::test]
    fn missing_source_is_not_found() {
        let resolver = MemResolver::default();
        let error = resolver.load(None, "nope.ron").unwrap_err();
        assert!(matches!(error, UiDocError::NotFound { .. }));
    }
}
