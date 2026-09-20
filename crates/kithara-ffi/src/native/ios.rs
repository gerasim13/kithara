use kithara::assets::StorageBackend;
use kithara_apple::foundation::prepare_playback_cache_directory;

pub(super) fn default_backend() -> StorageBackend {
    prepare_playback_cache_directory().map_or_else(StorageBackend::default, |root| {
        StorageBackend::Disk { root }
    })
}
