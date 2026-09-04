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
    pub bytes: Arc<[u8]>,
    pub uri: SourceUri,
}

pub trait SourceResolver {
    /// Loads `rel` as bytes, resolved against `base` on the same terms.
    ///
    /// A picture is not a document: a skin that names one reads it through
    /// this door rather than the one every text source comes through, because
    /// PNG bytes are not valid UTF-8 and would be refused on the way in.
    ///
    /// # Errors
    /// Returns [`UiDocError`] when the path escapes the root or is unavailable.
    fn bytes(&self, base: Option<&SourceUri>, rel: &str) -> Result<LoadedBytes, UiDocError>;

    /// Loads `rel`, resolved against the directory containing `base`.
    ///
    /// # Errors
    /// Returns [`UiDocError`] when the path escapes the root or is unavailable.
    fn load(&self, base: Option<&SourceUri>, rel: &str) -> Result<LoadedSource, UiDocError>;
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

pub(crate) fn resolve_uri(base: Option<&SourceUri>, rel: &str) -> Result<SourceUri, UiDocError> {
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
