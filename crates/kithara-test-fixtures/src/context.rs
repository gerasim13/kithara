#[cfg(feature = "hls")]
use std::io;
use std::{marker::PhantomData, path::Path};

#[cfg(feature = "hls")]
use crate::store;

/// The side-file store is reached by the families that package their own
/// segments; the rest produce their bytes in one piece and never open it.
pub(crate) struct BuildContext<'a> {
    #[cfg(feature = "hls")]
    namespace: &'a Path,
    #[cfg(feature = "hls")]
    asset_id: &'a str,
    marker: PhantomData<&'a Path>,
}

impl<'a> BuildContext<'a> {
    pub(crate) const fn new(namespace: &'a Path, asset_id: &'a str) -> Self {
        let _ = (namespace, asset_id);
        Self {
            #[cfg(feature = "hls")]
            namespace,
            #[cfg(feature = "hls")]
            asset_id,
            marker: PhantomData,
        }
    }

    #[cfg(feature = "hls")]
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
