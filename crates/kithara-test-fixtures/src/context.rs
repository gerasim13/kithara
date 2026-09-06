use std::{io, path::Path};

use crate::store;

pub(crate) struct BuildContext<'a> {
    asset_id: &'a str,
    namespace: &'a Path,
}

impl<'a> BuildContext<'a> {
    pub(crate) const fn new(namespace: &'a Path, asset_id: &'a str) -> Self {
        Self {
            asset_id,
            namespace,
        }
    }

    pub(crate) fn store(&self, key: &str, ext: &str, bytes: &[u8]) -> io::Result<String> {
        let id = store::asset_id(self.asset_id, key);
        if !store::has_entry(self.namespace, &id, ext) {
            let _lock = store::lock_entry(self.namespace, &id)?;
            if !store::has_entry(self.namespace, &id, ext) {
                store::write_entry(self.namespace, &id, ext, bytes)?;
            }
        }
        Ok(format!("{id}.{ext}"))
    }
}
