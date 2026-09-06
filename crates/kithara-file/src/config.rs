use std::{fmt, path::PathBuf};

use bon::Builder;
use kithara_assets::AssetStore;
use kithara_bufpool::{HasPool, PoolRegion};
use kithara_events::EventBus;
use kithara_net::Headers;
use kithara_platform::{CancelToken, time::Duration};
use kithara_stream::dl::Downloader;
use url::Url;

/// Source of a file stream: either a remote URL or a local path.
#[derive(Clone, Debug, derive_more::From, PartialEq, Eq)]
pub enum FileSrc {
    /// Remote file accessed via HTTP(S).
    Remote(Url),
    /// Local file accessed directly from disk.
    Local(PathBuf),
}

/// Configuration for file streaming.
///
/// Used with `Stream::<File<S>>::new(config)`.
#[derive(Builder)]
#[builder(on(String, into), start_fn = for_src)]
#[non_exhaustive]
pub struct FileConfig<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    /// File source (remote URL or local path).
    #[builder(start_fn)]
    pub src: FileSrc,
    /// Shared asset store used by local and remote sources.
    pub store: AssetStore<S>,
    /// Poll interval while a sibling `AssetStore` instance holds the
    /// atomic-chunked tmp for this file's canonical path. The default is short
    /// enough that the observed ~67 ms race window in
    /// `local_queue_playlist_behavior` resolves in a handful of ticks, long
    /// enough not to busy-spin a tokio worker.
    #[builder(default = Duration::from_millis(10))]
    pub tmp_claim_poll_interval: Duration,
    /// Event bus (optional - if not provided, one is created internally).
    #[builder(name = events)]
    pub bus: Option<EventBus>,
    /// Cancellation token for graceful shutdown.
    pub cancel: Option<CancelToken>,
    /// Optional cache discriminator.
    pub discriminator: Option<String>,
    /// Shared downloader (created lazily if not provided).
    pub downloader: Option<Downloader>,
    /// Explicit source-extension hint used before the URL-path extension.
    pub extension: Option<String>,
    /// Additional HTTP headers to include in all requests.
    pub headers: Option<Headers>,
    /// Max bytes the downloader may be ahead of the reader before it pauses.
    pub look_ahead_bytes: Option<u64>,
    /// Buffer-pool facade shared with storage and fallback transport.
    pub pools: PoolRegion<S>,
    /// Event bus channel capacity (used when `bus` is not provided).
    #[builder(default = kithara_events::DEFAULT_EVENT_BUS_CAPACITY)]
    pub event_channel_capacity: usize,
    /// Ring depth for the decode-core to shell reader-event hand-off. A decode
    /// pass emits at most one progress event per decoded chunk, so the default
    /// bounds the worst-case post-seek skip burst without blocking the decode
    /// core.
    #[builder(default = 256)]
    pub reader_event_capacity: usize,
}

impl<S> Clone for FileConfig<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    fn clone(&self) -> Self {
        Self {
            src: self.src.clone(),
            store: self.store.clone(),
            pools: self.pools.clone(),
            bus: self.bus.clone(),
            cancel: self.cancel.clone(),
            discriminator: self.discriminator.clone(),
            downloader: self.downloader.clone(),
            extension: self.extension.clone(),
            headers: self.headers.clone(),
            look_ahead_bytes: self.look_ahead_bytes,
            event_channel_capacity: self.event_channel_capacity,
            reader_event_capacity: self.reader_event_capacity,
            tmp_claim_poll_interval: self.tmp_claim_poll_interval,
        }
    }
}

impl<S> fmt::Debug for FileConfig<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileConfig")
            .field("src", &self.src)
            .field("bus", &self.bus)
            .field("cancel", &self.cancel)
            .field("headers", &self.headers)
            .field("look_ahead_bytes", &self.look_ahead_bytes)
            .field("extension", &self.extension)
            .field("discriminator", &self.discriminator)
            .field("pools", &self.pools)
            .field("store", &self.store)
            .field("event_channel_capacity", &self.event_channel_capacity)
            .field("reader_event_capacity", &self.reader_event_capacity)
            .field("tmp_claim_poll_interval", &self.tmp_claim_poll_interval)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use kithara_assets::{AssetStore, StorageBackend};
    use kithara_test_utils::kithara;

    use super::*;
    use crate::test_pools::{TestPools, pools};

    type TestConfig = FileConfig<TestPools>;
    type TestStore = AssetStore<TestPools>;

    fn test_src() -> FileSrc {
        FileSrc::Remote(Url::parse("http://example.com/audio.mp3").unwrap())
    }

    fn test_store() -> TestStore {
        AssetStore::builder(pools())
            .backend(StorageBackend::Memory)
            .build()
    }

    #[kithara::test]
    #[case(test_src())]
    #[case(FileSrc::Local(PathBuf::from("/tmp/song.mp3")))]
    fn test_file_config_for_src_preserves_source(#[case] src: FileSrc) {
        let config = FileConfig::for_src(src.clone())
            .store(test_store())
            .pools(pools())
            .build();

        assert_eq!(config.src, src);
        assert!(config.bus.is_none());
        assert!(config.cancel.is_none());
        if let FileSrc::Local(path) = &config.src {
            assert_eq!(path, Path::new("/tmp/song.mp3"));
        }
    }

    #[kithara::test]
    fn test_with_store() {
        let config = FileConfig::for_src(test_src())
            .store(test_store())
            .pools(pools())
            .build();

        assert!(config.bus.is_none());
    }

    fn apply_cancel(mut config: TestConfig) -> TestConfig {
        config.cancel = Some(CancelToken::never());
        config
    }

    fn apply_events(mut config: TestConfig) -> TestConfig {
        config.bus = Some(EventBus::new(32));
        config
    }

    fn apply_headers(mut config: TestConfig) -> TestConfig {
        let mut headers = Headers::default();
        headers.insert("Authorization", "Bearer token123");
        config.headers = Some(headers);
        config
    }

    fn has_cancel(config: &TestConfig) -> bool {
        config.cancel.is_some()
    }

    fn has_bus(config: &TestConfig) -> bool {
        config.bus.is_some()
    }

    fn has_auth_header(config: &TestConfig) -> bool {
        config.headers.as_ref().and_then(|h| h.get("Authorization")) == Some("Bearer token123")
    }

    #[kithara::test]
    #[case(apply_cancel, has_cancel)]
    #[case(apply_events, has_bus)]
    #[case(apply_headers, has_auth_header)]
    fn test_optional_setters_update_expected_field(
        #[case] apply: fn(TestConfig) -> TestConfig,
        #[case] check: fn(&TestConfig) -> bool,
    ) {
        let config = apply(
            FileConfig::for_src(test_src())
                .store(test_store())
                .pools(pools())
                .build(),
        );
        assert!(check(&config));
    }

    #[kithara::test]
    fn test_builder_chain() {
        let cancel = CancelToken::never();
        let bus = EventBus::new(32);

        let config = FileConfig::for_src(test_src())
            .store(test_store())
            .pools(pools())
            .cancel(cancel)
            .events(bus)
            .build();

        assert!(config.cancel.is_some());
        assert!(config.bus.is_some());
    }

    #[kithara::test]
    #[case("stream-a")]
    #[case("stream-b")]
    fn test_with_discriminator_sets_discriminator(#[case] name: &str) {
        let config = FileConfig::for_src(test_src())
            .store(test_store())
            .pools(pools())
            .discriminator(name)
            .build();
        assert_eq!(config.discriminator.as_deref(), Some(name));
    }

    #[kithara::test]
    fn test_debug_impl() {
        let config = FileConfig::for_src(test_src())
            .store(test_store())
            .pools(pools())
            .build();
        let debug_str = format!("{:?}", config);

        assert!(debug_str.contains("FileConfig"));
    }

    #[kithara::test]
    fn test_clone() {
        let bus = EventBus::new(32);
        let config = FileConfig::for_src(test_src())
            .store(test_store())
            .pools(pools())
            .events(bus)
            .build();

        let cloned = config.clone();

        assert!(config.bus.is_some());
        assert!(cloned.bus.is_some());
    }
}
