#![forbid(unsafe_code)]

use std::fmt;

use bon::Builder;
use kithara_abr::AbrMode;
use kithara_assets::{AssetStore, BytePool};
use kithara_drm::KeyProcessorRegistry;
use kithara_events::EventBus;
use kithara_net::{Headers, NetOptions};
use kithara_platform::CancelToken;
use kithara_stream::dl::Downloader;
use url::Url;

/// Encryption key handling configuration.
///
/// DRM key processing is routed through [`KeyProcessorRegistry`]. Concrete
/// resolvers decide which URLs they handle and return prepared requests without
/// exposing provider policy to HLS.
#[derive(Clone, Debug, Default, Builder)]
#[builder(state_mod(vis = "pub"))]
#[non_exhaustive]
pub struct KeyOptions {
    /// Ordered processor resolver registry. A URL not handled by any resolver
    /// uses the raw key as plain AES-128.
    pub key_registry: Option<KeyProcessorRegistry>,
}

/// Method used for lazy exact-size probes when a file-like decoder path needs
/// byte-accurate segment offsets and `#EXT-X-BYTERANGE` is absent.
///
/// `Head` is the spec-correct default. Some WAFs (notably zvuk's stage
/// `/drm/` path) drop `HEAD` bursts with a TCP close while still
/// happily serving `GET`s, so callers can switch the probe to a
/// single-byte ranged `GET` whose `Content-Range` header carries the
/// resource total.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum SizeProbeMethod {
    /// Issue `HEAD` requests (RFC-correct, what almost every CDN
    /// expects). The default.
    #[default]
    Head,
    /// Issue `GET` requests with `Range: bytes=0-0`; reads the
    /// resource total from the response's `Content-Range` header
    /// (or from `Content-Length` for non-206 responses). One byte
    /// of body per probe, but survives upstreams that reject `HEAD`.
    RangeGet,
}

/// Configuration for HLS streaming.
///
/// Used with `Stream::<Hls>::new(config)`.
#[derive(Clone, Builder)]
#[builder(start_fn = for_url)]
#[non_exhaustive]
pub struct HlsConfig {
    /// Master playlist URL.
    #[builder(start_fn)]
    pub url: Url,
    /// Initial ABR mode.
    #[builder(default)]
    pub initial_abr_mode: AbrMode,
    /// Shared asset store.
    pub store: AssetStore,
    /// Buffer pool shared across all components.
    #[builder(default = BytePool::default())]
    pub pool: BytePool,
    /// Encryption key handling configuration.
    #[builder(default)]
    pub keys: KeyOptions,
    /// Net options (idle/stall `inactivity_timeout`, `retry_policy`,
    /// compression) for the HTTP client built when no [`downloader`] is
    /// injected. Ignored when [`downloader`] is provided — the injected
    /// downloader already carries its own client. Defaults to
    /// [`NetOptions::default`]; lower the `inactivity_timeout` to bound a
    /// withheld-body fetch sooner (the net resilient body owns the stall
    /// and retries, then settles the segment terminally).
    ///
    /// [`downloader`]: Self::downloader
    #[builder(default)]
    pub net_options: NetOptions,
    /// Base URL for resolving relative playlist/segment URLs.
    pub base_url: Option<Url>,
    /// Event bus (optional - if not provided, one is created internally).
    #[builder(name = events)]
    pub bus: Option<EventBus>,
    /// Cancellation token for graceful shutdown. The master `CancelToken` whose
    /// shared atomic mirror reaches [`HlsCoord`](crate::stream::HlsCoord)'s
    /// lock-free `is_cancelled()` read on the produce-core; the async-only
    /// downloader / net / asset paths derive children from its inner
    /// [`CancelToken`](kithara_platform::CancelToken).
    pub cancel: Option<CancelToken>,
    /// Optional cache discriminator.
    pub discriminator: Option<String>,
    /// Shared downloader (created lazily if not provided).
    pub downloader: Option<Downloader>,
    /// Additional HTTP headers to include in all requests.
    pub headers: Option<Headers>,
    /// Max bytes the downloader may be ahead of the reader before it pauses.
    /// `None` falls back to [`HlsConfig::DEFAULT_LOOK_AHEAD_BYTES`] (~2 `MiB`)
    /// at the consumer site — production HLS streams need a downloader
    /// backpressure cap. Pass `Some(0)` to disable the cap explicitly.
    pub look_ahead_bytes: Option<u64>,
    /// Method used by on-demand exact-size probes. Segment-aware fMP4 decode
    /// never issues these probes; file-like paths use them after a seek needs
    /// exact prefix offsets.
    #[builder(default)]
    pub size_probe_method: SizeProbeMethod,
    /// Acquire attempts a planned segment slot gets before the dispatch
    /// settles it terminally. A requeue is re-dispatched on the peer's next
    /// poll, so this counts dispatch rounds, not wall-clock time. A tmp held
    /// by a live sibling writer is exempt — that holder always settles and
    /// releases, so its retry resolves on its own.
    #[builder(default = HlsConfig::DEFAULT_ACQUIRE_ATTEMPT_BUDGET)]
    pub acquire_attempt_budget: u8,
    /// Max segments to download per step.
    #[builder(default = HlsConfig::DEFAULT_DOWNLOAD_BATCH_SIZE)]
    pub download_batch_size: usize,
    /// Maximum media-segment prefetch window for ephemeral HLS stores.
    /// The effective maximum is never lower than
    /// [`Self::ephemeral_cache_min_media_window`].
    #[builder(default = HlsConfig::DEFAULT_EPHEMERAL_CACHE_MAX_MEDIA_WINDOW)]
    pub ephemeral_cache_max_media_window: usize,
    /// Minimum media-segment prefetch window for ephemeral HLS stores after
    /// applying [`Self::ephemeral_cache_non_media_reserve`].
    #[builder(default = HlsConfig::DEFAULT_EPHEMERAL_CACHE_MIN_MEDIA_WINDOW)]
    pub ephemeral_cache_min_media_window: usize,
    /// Number of non-media HLS cache entries reserved when deriving the
    /// ephemeral media prefetch window from the store cache capacity.
    #[builder(default = HlsConfig::DEFAULT_EPHEMERAL_CACHE_NON_MEDIA_RESERVE)]
    pub ephemeral_cache_non_media_reserve: usize,
    /// Capacity of the event bus channel (used when `bus` is not provided).
    #[builder(default = kithara_events::DEFAULT_EVENT_BUS_CAPACITY)]
    pub event_channel_capacity: usize,
}

impl fmt::Debug for HlsConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HlsConfig")
            .field("initial_abr_mode", &self.initial_abr_mode)
            .field("keys", &self.keys)
            .field("base_url", &self.base_url)
            .field("bus", &self.bus)
            .field("cancel", &self.cancel)
            .field("headers", &self.headers)
            .field("look_ahead_bytes", &self.look_ahead_bytes)
            .field(
                "ephemeral_cache_non_media_reserve",
                &self.ephemeral_cache_non_media_reserve,
            )
            .field(
                "ephemeral_cache_min_media_window",
                &self.ephemeral_cache_min_media_window,
            )
            .field(
                "ephemeral_cache_max_media_window",
                &self.ephemeral_cache_max_media_window,
            )
            .field("discriminator", &self.discriminator)
            .field("pool", &self.pool)
            .field("store", &self.store)
            .field("url", &self.url)
            .field("download_batch_size", &self.download_batch_size)
            .field("event_channel_capacity", &self.event_channel_capacity)
            .field("size_probe_method", &self.size_probe_method)
            .field("net_options", &self.net_options)
            .finish_non_exhaustive()
    }
}

impl HlsConfig {
    /// Default [`Self::acquire_attempt_budget`]. Enough rounds for an
    /// obstruction another task is already clearing to disappear, few enough
    /// that a standing one reaches the reader instead of parking it.
    pub const DEFAULT_ACQUIRE_ATTEMPT_BUDGET: u8 = 3;
    /// Default [`Self::download_batch_size`]. Three segments keep the fetcher
    /// busy across one round-trip without planning further ahead than a
    /// look-ahead cap would allow anyway.
    pub const DEFAULT_DOWNLOAD_BATCH_SIZE: usize = 3;
    /// Per-stream media window for a shared 128-entry cache. Two concurrent
    /// streams each retain 60 media and four non-media entries.
    pub const DEFAULT_EPHEMERAL_CACHE_MAX_MEDIA_WINDOW: usize = 60;
    pub const DEFAULT_EPHEMERAL_CACHE_MIN_MEDIA_WINDOW: usize = 3;
    pub const DEFAULT_EPHEMERAL_CACHE_NON_MEDIA_RESERVE: usize = 4;
    /// Default `look_ahead_bytes` cap (~2 `MiB`). Production HLS streams
    /// need a downloader backpressure cap so an idle reader does not
    /// drain the whole playlist into cache.
    pub const DEFAULT_LOOK_AHEAD_BYTES: u64 = 2 * 1024 * 1024;
}
