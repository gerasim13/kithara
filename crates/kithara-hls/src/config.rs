#![forbid(unsafe_code)]

use std::fmt;

use bon::Builder;
use kithara_abr::AbrMode;
use kithara_assets::AssetStore;
use kithara_bufpool::{HasPool, PoolRegion};
use kithara_drm::KeyProcessorRegistry;
use kithara_events::EventBus;
use kithara_net::{Headers, NetOptions};
use kithara_platform::CancelToken;
use kithara_stream::dl::Downloader;
use struct_patch::Patch;
use url::Url;

/// Enough rounds for an obstruction another task is already clearing to
/// disappear, few enough that a standing one reaches the reader instead of
/// parking it.
pub(crate) const DEFAULT_ACQUIRE_ATTEMPT_BUDGET: u8 = 3;
pub(crate) const DEFAULT_EPHEMERAL_CACHE_MAX_MEDIA_WINDOW: usize = 60;
pub(crate) const DEFAULT_EPHEMERAL_CACHE_MIN_MEDIA_WINDOW: usize = 3;
pub(crate) const DEFAULT_EPHEMERAL_CACHE_NON_MEDIA_RESERVE: usize = 4;
pub(crate) const DEFAULT_DOWNLOAD_BATCH_SIZE: usize = 3;
/// Production HLS streams need a downloader backpressure cap so an idle reader
/// does not drain the whole playlist into cache.
pub(crate) const DEFAULT_LOOK_AHEAD_BYTES: u64 = 2 * 1024 * 1024;

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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
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

/// Streaming knobs a configuration document can override. Extracted out of
/// [`HlsConfig`] so a document reaches exactly these tunables and never the
/// per-call wiring (`store`, `pools`, `downloader`, ...) or per-stream input
/// (`url`, `base_url`, `discriminator`, `headers`, `initial_abr_mode`) that
/// stay on [`HlsConfig`] itself.
#[derive(Clone, Debug, Builder, Patch)]
#[builder(state_mod(vis = "pub"))]
#[patch(name = "HlsSettingsPatch")]
#[patch(attribute(derive(Clone, Debug, Default, serde::Deserialize)))]
#[patch(attribute(serde(default, deny_unknown_fields)))]
#[patch(attribute(non_exhaustive))]
#[non_exhaustive]
pub struct HlsSettings {
    /// Net options (idle/stall `inactivity_timeout`, `retry_policy`,
    /// compression) for the HTTP client built when no [`downloader`] is
    /// injected. Ignored when [`downloader`] is provided — the injected
    /// downloader already carries its own client. Defaults to
    /// [`NetOptions::default`]; lower the `inactivity_timeout` to bound a
    /// withheld-body fetch sooner (the net resilient body owns the stall
    /// and retries, then settles the segment terminally).
    ///
    /// A document cannot name this: an embedder that reaches a document also
    /// injects a downloader, so the value would configure nothing. It carries
    /// `#[patch(skip)]` for that reason.
    ///
    /// [`downloader`]: HlsConfig::downloader
    #[builder(default)]
    #[patch(skip)]
    pub net_options: NetOptions,
    /// Method used by on-demand exact-size probes. Segment-aware fMP4 decode
    /// never issues these probes; file-like paths use them after a seek needs
    /// exact prefix offsets.
    #[builder(default)]
    pub size_probe_method: SizeProbeMethod,
    /// Max segments to download per step. Three keep the fetcher busy across
    /// one round-trip without planning further ahead than a look-ahead cap
    /// would allow anyway.
    #[builder(default = DEFAULT_DOWNLOAD_BATCH_SIZE)]
    pub download_batch_size: usize,
    /// Acquire attempts a planned segment slot gets before the dispatch
    /// settles it terminally. A requeue is re-dispatched on the peer's next
    /// poll, so this counts dispatch rounds, not wall-clock time. A tmp held
    /// by a live sibling writer is exempt — that holder always settles and
    /// releases, so its retry resolves on its own.
    #[builder(default = DEFAULT_ACQUIRE_ATTEMPT_BUDGET)]
    pub acquire_attempt_budget: u8,
    /// Maximum media-segment prefetch window for ephemeral HLS stores.
    /// The effective maximum is never lower than
    /// [`Self::ephemeral_cache_min_media_window`]. Sized for a shared
    /// 128-entry cache: two concurrent streams each retain 60 media and four
    /// non-media entries.
    #[builder(default = DEFAULT_EPHEMERAL_CACHE_MAX_MEDIA_WINDOW)]
    pub ephemeral_cache_max_media_window: usize,
    /// Minimum media-segment prefetch window for ephemeral HLS stores after
    /// applying [`Self::ephemeral_cache_non_media_reserve`].
    #[builder(default = DEFAULT_EPHEMERAL_CACHE_MIN_MEDIA_WINDOW)]
    pub ephemeral_cache_min_media_window: usize,
    /// Number of non-media HLS cache entries reserved when deriving the
    /// ephemeral media prefetch window from the store cache capacity.
    #[builder(default = DEFAULT_EPHEMERAL_CACHE_NON_MEDIA_RESERVE)]
    pub ephemeral_cache_non_media_reserve: usize,
    /// Capacity of the event bus channel (used when `bus` is not provided).
    #[builder(default = kithara_events::DEFAULT_EVENT_BUS_CAPACITY)]
    pub event_channel_capacity: usize,
    /// Max bytes the downloader may be ahead of the reader before it pauses.
    /// `None` falls back to `DEFAULT_LOOK_AHEAD_BYTES` (~2 `MiB`)
    /// at the consumer site — production HLS streams need a downloader
    /// backpressure cap. Pass `Some(0)` to disable the cap explicitly.
    #[patch(skip_wrap)]
    pub look_ahead_bytes: Option<u64>,
}

impl Default for HlsSettings {
    fn default() -> Self {
        Self::builder().build()
    }
}

/// Configuration for HLS streaming.
///
/// Used with `Stream::<Hls<S>>::new(config)`.
#[derive(Builder)]
#[builder(start_fn = for_url)]
#[non_exhaustive]
pub struct HlsConfig<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    /// Master playlist URL.
    #[builder(start_fn)]
    pub url: Url,
    /// Initial ABR mode.
    #[builder(default)]
    pub initial_abr_mode: AbrMode,
    /// Shared asset store.
    pub store: AssetStore<S>,
    /// Buffer-pool facade shared across all components.
    pub pools: PoolRegion<S>,
    /// Encryption key handling configuration.
    #[builder(default)]
    pub keys: KeyOptions,
    /// Streaming knobs a configuration document can override. See
    /// [`HlsSettings`] for what a document may say.
    #[builder(default)]
    pub settings: HlsSettings,
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
}

impl<S> Clone for HlsConfig<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    fn clone(&self) -> Self {
        Self {
            url: self.url.clone(),
            initial_abr_mode: self.initial_abr_mode,
            store: self.store.clone(),
            pools: self.pools.clone(),
            keys: self.keys.clone(),
            settings: self.settings.clone(),
            base_url: self.base_url.clone(),
            bus: self.bus.clone(),
            cancel: self.cancel.clone(),
            discriminator: self.discriminator.clone(),
            downloader: self.downloader.clone(),
            headers: self.headers.clone(),
        }
    }
}

impl<S> fmt::Debug for HlsConfig<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HlsConfig")
            .field("initial_abr_mode", &self.initial_abr_mode)
            .field("keys", &self.keys)
            .field("settings", &self.settings)
            .field("base_url", &self.base_url)
            .field("bus", &self.bus)
            .field("cancel", &self.cancel)
            .field("headers", &self.headers)
            .field("discriminator", &self.discriminator)
            .field("pools", &self.pools)
            .field("store", &self.store)
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use kithara_assets::StorageBackend;
    use kithara_test_utils::kithara;
    use struct_patch::Patch as _;

    use super::{AssetStore, HlsConfig, HlsSettings, HlsSettingsPatch, SizeProbeMethod};

    /// The one knob `kithara-app` reads out of this patch resolves through
    /// `SizeProbeMethod::default()` whenever a document stays silent, so the
    /// default is part of the contract rather than an implementation detail.
    #[kithara::test(native, flash(false))]
    fn a_silent_document_probes_with_head() {
        assert_eq!(
            HlsSettings::default().size_probe_method,
            SizeProbeMethod::Head
        );
    }

    #[kithara::test(native, flash(false))]
    fn a_document_sets_the_batch_size_and_leaves_the_window() {
        let patch: HlsSettingsPatch =
            serde_yaml_ng::from_str("download_batch_size: 6\n").expect("the document types");
        let mut settings = HlsSettings::default();
        let window = settings.ephemeral_cache_max_media_window;

        settings.apply(patch);

        assert_eq!(settings.download_batch_size, 6);
        assert_eq!(settings.ephemeral_cache_max_media_window, window);
    }

    #[kithara::test(native, flash(false))]
    fn an_already_optional_knob_takes_a_bare_number_from_the_document() {
        let patch: HlsSettingsPatch =
            serde_yaml_ng::from_str("look_ahead_bytes: 5000000\n").expect("the document types");
        let mut settings = HlsSettings::builder().event_channel_capacity(4_096).build();

        settings.apply(patch);

        assert_eq!(
            settings.look_ahead_bytes,
            Some(5_000_000),
            "an `Option<u64>` field carries `skip_wrap`, so the document names the number bare"
        );
        assert_eq!(
            settings.event_channel_capacity, 4_096,
            "a silent field must keep its value"
        );
    }

    /// The proof `net_options` is absent from the document rather than parsed
    /// and dropped: an embedder that reaches a document injects its own
    /// downloader, which makes the field dead, so naming it must fail loudly.
    #[kithara::test(native, flash(false))]
    fn a_net_options_key_is_refused() {
        let error =
            serde_yaml_ng::from_str::<HlsSettingsPatch>("net_options:\n  is_insecure: true\n")
                .expect_err("net options belong to the embedder's own `net` section");

        assert!(error.to_string().contains("net_options"), "{error}");
    }

    #[kithara::test(native, flash(false))]
    fn a_document_knob_reaches_the_built_config() {
        let patch: HlsSettingsPatch =
            serde_yaml_ng::from_str("download_batch_size: 7\n").expect("the document types");
        let mut settings = HlsSettings::default();
        settings.apply(patch);

        let config = HlsConfig::<crate::test_pools::TestPools>::for_url(
            "https://example.com/master.m3u8"
                .parse()
                .expect("master url"),
        )
        .store(
            AssetStore::builder(crate::test_pools::pools())
                .backend(StorageBackend::Memory)
                .build(),
        )
        .pools(crate::test_pools::pools())
        .settings(settings)
        .build();

        assert_eq!(config.settings.download_batch_size, 7);
    }

    #[kithara::test(native, flash(false))]
    fn an_unknown_field_is_rejected_and_named() {
        let error = serde_yaml_ng::from_str::<HlsSettingsPatch>("download_batch_sizes: 6\n")
            .expect_err("a typo must not be silently ignored");

        assert!(
            error.to_string().contains("download_batch_sizes"),
            "{error}"
        );
    }
}
