use kithara::assets::StorageBackend;
use kithara_apple::foundation::prepare_documents_directory;

pub(super) fn default_backend() -> StorageBackend {
    prepare_documents_directory("Files/Kithara").map_or_else(StorageBackend::default, |root| {
        StorageBackend::Disk { root }
    })
}
