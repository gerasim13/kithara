use kithara::assets::StorageBackend;

#[cfg(target_os = "ios")]
pub(super) fn default_backend() -> StorageBackend {
    kithara_apple::foundation::filesystem::prepare_documents_directory("Files/Kithara")
        .map_or_else(StorageBackend::default, |root| StorageBackend::Disk {
            root,
        })
}

#[cfg(not(target_os = "ios"))]
pub(super) fn default_backend() -> StorageBackend {
    StorageBackend::default()
}

#[cfg(all(test, not(target_os = "ios")))]
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
