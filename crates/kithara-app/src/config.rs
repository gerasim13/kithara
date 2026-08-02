use std::fmt;

use bon::Builder;
use kithara::{
    assets::{AssetStore, AssetStoreBuilder, BytePool, FlushHub, StorageBackend},
    audio::analysis::BeatAnalysisConfig,
    bufpool::PcmPool,
    hls::SizeProbeMethod,
    play::policy::DomainKeyPolicy,
    prelude::PlaybackResamplerBackend,
    stream::dl::Downloader,
};
use kithara_drm::KeyProcessorRegistry;
use kithara_platform::{CancelToken, sync::Arc};
use url::Url;

use crate::{baked, theme::Palette};

/// App-owned snapshot of one DRM policy and its ordinary resolver registry.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct AppDrm {
    policy: Arc<DomainKeyPolicy>,
    registry: KeyProcessorRegistry,
}

impl AppDrm {
    /// Register one immutable domain policy and retain the same policy for
    /// resource-header selection.
    #[must_use]
    pub fn new(policy: DomainKeyPolicy) -> Self {
        let policy = Arc::new(policy);
        let mut registry = KeyProcessorRegistry::new();
        registry.register(policy.clone());
        Self { policy, registry }
    }

    /// Return the opaque key-request registry.
    #[must_use]
    pub fn registry(&self) -> &KeyProcessorRegistry {
        &self.registry
    }

    /// Return resource headers selected by the same registered policy.
    #[must_use]
    pub fn resource_headers(&self, url: &Url) -> Option<kithara::net::Headers> {
        self.policy.resource_headers(url)
    }
}

impl Default for AppDrm {
    fn default() -> Self {
        Self::new(baked::build_baked_drm_policy())
    }
}

/// Application configuration passed to frontends.
///
/// Shared owners and the downloader are mandatory; product knobs default to
/// values baked at compile time from `app.toml`.
#[derive(Clone, Builder)]
#[builder(state_mod(vis = "pub"))]
#[non_exhaustive]
pub struct AppConfig {
    /// App-owned DRM policy and its opaque key-request registry.
    #[builder(default)]
    pub drm: AppDrm,
    /// App-wide shared asset store.
    pub store: AssetStore,
    /// Source beat-analysis tunables.
    #[builder(default)]
    pub beat_analysis: BeatAnalysisConfig<PlaybackResamplerBackend>,
    /// App-wide shared byte pool for network and cache buffers.
    pub byte_pool: BytePool,
    /// App master cancel. Single owner for the whole app subtree; the
    /// queue, player, stores, and UI listener all derive children from
    /// it (see `main.rs`). The chain flag reaches the audio worker and HLS
    /// coord lock-free `is_cancelled()` reads; every subsystem derives its
    /// own [`CancelToken::child`] from this consumer-top master.
    pub shutdown: CancelToken,
    /// Shared HTTP downloader for every track.
    pub downloader: Downloader,
    /// Color palette for the UI.
    #[builder(default)]
    pub palette: Palette,
    /// App-wide shared PCM pool for playback and track analysis.
    pub pcm_pool: PcmPool,
    /// HLS size-estimation probe strategy (see
    /// [`kithara::hls::SizeProbeMethod`]).
    #[builder(default = baked::BAKED_SIZE_PROBE_METHOD)]
    pub size_probe_method: SizeProbeMethod,
    /// Log filter directives.
    #[builder(default)]
    pub log_directives: Vec<String>,
    /// Audio file URLs or paths to play.
    #[builder(default = default_tracks())]
    pub tracks: Vec<String>,
    /// Accept invalid TLS certificates. Test servers only.
    #[builder(default = baked::BAKED_SHOULD_ACCEPT_INVALID_CERTS)]
    pub should_accept_invalid_certs: bool,
    /// Crossfade duration in seconds.
    #[builder(default = baked::BAKED_CROSSFADE_SECONDS)]
    pub crossfade_seconds: f32,
}

fn default_tracks() -> Vec<String> {
    baked::BAKED_TRACKS
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

impl fmt::Debug for AppConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppConfig")
            .field("drm", &self.drm)
            .field("palette", &self.palette)
            .field("log_directives", &self.log_directives)
            .field("tracks", &self.tracks)
            .field("byte_pool", &self.byte_pool)
            .field("pcm_pool", &self.pcm_pool)
            .field(
                "should_accept_invalid_certs",
                &self.should_accept_invalid_certs,
            )
            .field("crossfade_seconds", &self.crossfade_seconds)
            .field("beat_analysis", &self.beat_analysis)
            .field("size_probe_method", &self.size_probe_method)
            .finish_non_exhaustive()
    }
}

impl AppConfig {
    /// Create a default config around the injected app-wide owners.
    #[must_use]
    pub fn new(
        downloader: Downloader,
        flush_hub: Arc<FlushHub>,
        cancel: CancelToken,
        byte_pool: BytePool,
        pcm_pool: PcmPool,
    ) -> Self {
        let store = AssetStoreBuilder::default()
            .cancel(cancel.child())
            .backend(StorageBackend::default())
            .pool(byte_pool.clone())
            .flush_hub(flush_hub)
            .layouts(baked::build_baked_asset_layouts())
            .build();
        Self::builder()
            .downloader(downloader)
            .shutdown(cancel)
            .byte_pool(byte_pool)
            .pcm_pool(pcm_pool)
            .store(store)
            .build()
    }
}
