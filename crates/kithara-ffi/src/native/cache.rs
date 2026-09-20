use kithara::assets::StorageBackend;

pub(super) fn default_backend() -> StorageBackend {
    StorageBackend::default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset::FfiAssetStore;

    #[kithara::test]
    fn platform_default_root_is_preserved() {
        let store = FfiAssetStore::for_test();
        let StorageBackend::Disk { root } = StorageBackend::default() else {
            panic!("native storage defaults to disk");
        };
        assert_eq!(store.handle().root_dir(), root);
    }
}
