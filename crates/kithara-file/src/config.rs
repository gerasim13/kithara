use std::{fmt, path::PathBuf};

use bon::Builder;
use kithara_assets::{AssetStore, BytePool, ProcessCtx, ResourceKey};
use kithara_events::EventBus;
use kithara_net::{Headers, NetError};
use kithara_platform::{CancelToken, sync::Arc};
use kithara_stream::{Activity, dl::FetchScope};
use url::Url;

/// Source of a file stream.
#[derive(Clone, Debug, derive_more::From, PartialEq, Eq)]
#[non_exhaustive]
pub enum FileSrc {
    /// Remote file accessed via HTTP(S). The cache key is derived from the
    /// URL.
    Remote(Url),
    /// Local file accessed directly from disk.
    Local(PathBuf),
    /// Storage resource already acquired and written by another owner.
    Attached(ResourceKey),
    /// Remote file whose cache key is owned by the caller. Used where the
    /// key belongs to another layout — an HLS segment is keyed by its
    /// variant's scope, not by its URL.
    Resource {
        /// Caller-minted key the bytes are fetched into.
        key: ResourceKey,
        /// Where the bytes come from.
        url: Url,
    },
}

/// How a fetch ended, reported once through
/// [`FileConfig::on_fetch_complete`].
///
/// This says what happened to the resource, never what to do next: the
/// retry policy belongs to whoever owns the key.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum FetchOutcome {
    /// The resource is committed. `final_len` is the length read back after
    /// the commit — a [`ProcessCtx`] pass can shrink it — or `None` when a
    /// racing writer committed the resource and its length is not known
    /// here.
    Committed {
        /// On-disk length after commit.
        final_len: Option<u64>,
    },
    /// The transfer failed and the resource was failed and evicted.
    Failed {
        /// What the transport reported. Classify it with
        /// [`NetError::retryability`].
        error: NetError,
    },
    /// The bytes arrived but the commit itself failed — a processing pass
    /// rejected them, so the resource was failed and evicted.
    CommitFailed {
        /// What the commit reported.
        reason: String,
    },
    /// The fetch was cancelled. `committed` is `true` when the resource was
    /// already committed by another writer, so the bytes on disk are final
    /// and this fetch's cancel says nothing about them.
    Cancelled {
        /// Whether the resource is committed despite the cancel.
        committed: bool,
    },
}

/// Reports the terminal [`FetchOutcome`] of a fetching source.
///
/// Called at most once, from the downloader thread that settles the fetch.
/// A caller that must consume something on the callback (a one-shot claim)
/// holds it behind its own `Option`.
pub type FetchCompleteFn = Arc<dyn Fn(FetchOutcome) + Send + Sync>;

/// Reports that an in-flight request crossed the downloader's soft timeout.
///
/// Not terminal: the fetch is still running. A caller that schedules across
/// several sources (an HLS variant deciding whether to escape a stalled
/// one) learns here that this one is not making progress.
pub type SlowFn = Arc<dyn Fn() + Send + Sync>;

/// Configuration for file streaming.
///
/// Used with `Stream::<File>::new(config)`.
#[derive(Clone, Builder)]
#[builder(state_mod(vis = "pub"))]
#[non_exhaustive]
pub struct FileConfig {
    /// File source.
    pub src: FileSrc,
    /// Event bus (optional - if not provided, one is created internally).
    #[builder(name = events)]
    pub bus: Option<EventBus>,
    /// Cancellation token for graceful shutdown.
    pub cancel: Option<CancelToken>,
    /// Where this file registers to fetch: the shared pool for a file that
    /// downloads for itself, or another peer's handle when the file is one
    /// piece of a larger download — an HLS segment fetching inside its
    /// variant. Registering in a branch is what ranks the fetch inside its
    /// parent, credits its bytes to the parent's ABR identity, and ends the
    /// registration when the parent goes away. A pool of its own is created
    /// when this is not provided.
    pub fetch: Option<Arc<dyn FetchScope>>,
    /// Additional HTTP headers to include in all requests.
    pub headers: Option<Headers>,
    /// Max bytes the downloader may be ahead of the reader before it pauses.
    pub look_ahead_bytes: Option<u64>,
    /// Explicit source-extension hint used before the URL-path extension.
    pub extension: Option<String>,
    /// Optional cache discriminator.
    pub discriminator: Option<String>,
    /// Whether the consumer of these bytes is active right now. Decides the
    /// fetch's queue priority; a source left to itself is never active and
    /// so never outranks one that is.
    pub activity: Option<Arc<dyn Activity>>,
    /// Reports how the fetch ended. Only a fetching source
    /// ([`FileSrc::Remote`] / [`FileSrc::Resource`]) reports; a local or
    /// attached source never fetches.
    pub on_fetch_complete: Option<FetchCompleteFn>,
    /// Called when an in-flight request crosses the downloader's soft
    /// timeout. Fires per request, not per source.
    pub on_slow: Option<SlowFn>,
    /// Shared byte pool for cache and fallback network buffers.
    pub pool: Option<BytePool>,
    /// Transform applied to the fetched bytes when the resource commits —
    /// decryption for an encrypted source. Opaque here: the context is built
    /// by whoever owns the scheme, so this crate stays cipher-agnostic.
    pub process: Option<ProcessCtx>,
    /// Shared asset store used by local and remote sources.
    pub store: AssetStore,
    /// Event bus channel capacity (used when `bus` is not provided).
    #[builder(default = kithara_events::DEFAULT_EVENT_BUS_CAPACITY)]
    pub event_channel_capacity: usize,
}

impl fmt::Debug for FileConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileConfig")
            .field("src", &self.src)
            .field("bus", &self.bus)
            .field("cancel", &self.cancel)
            .field("headers", &self.headers)
            .field("look_ahead_bytes", &self.look_ahead_bytes)
            .field("extension", &self.extension)
            .field("discriminator", &self.discriminator)
            .field("pool", &self.pool)
            .field("store", &self.store)
            .field("event_channel_capacity", &self.event_channel_capacity)
            .finish_non_exhaustive()
    }
}

impl FileConfig {
    /// Create new file config with source.
    #[must_use]
    pub fn new(src: FileSrc, store: AssetStore) -> Self {
        Self::for_src(src).store(store).build()
    }

    /// Chainable counterpart to [`FileConfig::new`].
    pub fn for_src(src: FileSrc) -> FileConfigBuilder<file_config_builder::SetSrc> {
        Self::builder().src(src)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use kithara_assets::{AssetStoreBuilder, StorageBackend};
    use kithara_test_utils::kithara;

    use super::*;

    fn test_src() -> FileSrc {
        FileSrc::Remote(Url::parse("http://example.com/audio.mp3").unwrap())
    }

    fn test_store() -> AssetStore {
        AssetStoreBuilder::default()
            .backend(StorageBackend::Memory)
            .build()
    }

    #[kithara::test]
    #[case(test_src())]
    #[case(FileSrc::Local(PathBuf::from("/tmp/song.mp3")))]
    #[case(FileSrc::Attached(
        test_store()
            .scope::<crate::File>(&kithara_assets::AssetSource::Remote {
                url: Url::parse("http://example.com/audio.mp3").unwrap(),
                discriminator: None,
            })
            .unwrap()
            .key(&kithara_assets::AssetResource::Source {
                extension: "mp3".to_string(),
            })
            .unwrap()
    ))]
    fn test_file_config_new_preserves_source(#[case] src: FileSrc) {
        let config = FileConfig::new(src.clone(), test_store());

        assert_eq!(config.src, src);
        assert!(config.bus.is_none());
        assert!(config.cancel.is_none());
        if let FileSrc::Local(path) = &config.src {
            assert_eq!(path, Path::new("/tmp/song.mp3"));
        }
    }

    #[kithara::test]
    fn test_with_store() {
        let config = FileConfig::for_src(test_src()).store(test_store()).build();

        assert!(config.bus.is_none());
    }

    fn apply_cancel(mut config: FileConfig) -> FileConfig {
        config.cancel = Some(CancelToken::never());
        config
    }

    fn apply_events(mut config: FileConfig) -> FileConfig {
        config.bus = Some(EventBus::new(32));
        config
    }

    fn apply_headers(mut config: FileConfig) -> FileConfig {
        let mut headers = Headers::new();
        headers.insert("Authorization", "Bearer token123");
        config.headers = Some(headers);
        config
    }

    fn has_cancel(config: &FileConfig) -> bool {
        config.cancel.is_some()
    }

    fn has_bus(config: &FileConfig) -> bool {
        config.bus.is_some()
    }

    fn has_auth_header(config: &FileConfig) -> bool {
        config.headers.as_ref().and_then(|h| h.get("Authorization")) == Some("Bearer token123")
    }

    #[kithara::test]
    #[case(apply_cancel, has_cancel)]
    #[case(apply_events, has_bus)]
    #[case(apply_headers, has_auth_header)]
    fn test_optional_setters_update_expected_field(
        #[case] apply: fn(FileConfig) -> FileConfig,
        #[case] check: fn(&FileConfig) -> bool,
    ) {
        let config = apply(FileConfig::new(test_src(), test_store()));
        assert!(check(&config));
    }

    #[kithara::test]
    fn test_builder_chain() {
        let cancel = CancelToken::never();
        let bus = EventBus::new(32);

        let config = FileConfig::for_src(test_src())
            .store(test_store())
            .cancel(cancel.clone())
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
            .discriminator(name.to_string())
            .build();
        assert_eq!(config.discriminator.as_deref(), Some(name));
    }

    #[kithara::test]
    fn test_debug_impl() {
        let config = FileConfig::new(test_src(), test_store());
        let debug_str = format!("{:?}", config);

        assert!(debug_str.contains("FileConfig"));
    }

    #[kithara::test]
    fn test_clone() {
        let bus = EventBus::new(32);
        let config = FileConfig::for_src(test_src())
            .store(test_store())
            .events(bus)
            .build();

        let cloned = config.clone();

        assert!(cloned.bus.is_some());
    }
}
