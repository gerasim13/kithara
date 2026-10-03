use std::collections::BTreeMap;

use kithara_platform::sync::Arc;

use crate::{
    error::UiDocError,
    ids::SourceUri,
    module::ModuleDoc,
    source::{
        resolve_uri,
        uri::{LoadedBytes, LoadedModule, LoadedSource, ModuleSource, SourceResolver},
    },
};

#[derive(Debug, Default)]
pub struct MemResolver {
    blobs: BTreeMap<String, Arc<[u8]>>,
    files: BTreeMap<String, ModuleSource>,
}

impl MemResolver {
    pub fn insert(&mut self, path: &str, text: &str) {
        self.files
            .insert(path.to_owned(), ModuleSource::Text(text.to_owned()));
    }

    /// Stores a ready module at a package-relative path.
    pub fn insert_module(&mut self, path: &str, document: ModuleDoc) {
        self.files
            .insert(path.to_owned(), ModuleSource::Document(Box::new(document)));
    }

    /// Adds a source that is not text, such as a picture a skin names.
    pub fn insert_bytes(&mut self, path: &str, bytes: &[u8]) {
        self.blobs.insert(path.to_owned(), Arc::from(bytes));
    }
}

impl SourceResolver for MemResolver {
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

    fn module(&self, base: Option<&SourceUri>, rel: &str) -> Result<LoadedModule, UiDocError> {
        let uri = resolve_uri(base, rel)?;
        let source = self
            .files
            .get(&uri.0)
            .cloned()
            .ok_or_else(|| UiDocError::NotFound {
                origin: base.cloned().unwrap_or_else(|| uri.clone()),
                rel: rel.to_owned(),
            })?;
        Ok(LoadedModule { uri, source })
    }

    fn load(&self, base: Option<&SourceUri>, rel: &str) -> Result<LoadedSource, UiDocError> {
        let loaded = self.module(base, rel)?;
        match loaded.source {
            ModuleSource::Text(text) => Ok(LoadedSource {
                uri: loaded.uri,
                text,
            }),
            ModuleSource::Document(_) => Err(UiDocError::WrongDocKind {
                origin: loaded.uri,
                expected: "text source",
                found: "module document",
            }),
        }
    }
}
