use std::{fmt, num::NonZeroU32, path::PathBuf};

use bon::Builder;
use kithara::{
    analysis::BeatAnalysisConfig, hls::SizeProbeMethod, play::policy::DomainKeyPolicy,
    prelude::PlaybackResamplerBackend, stream::dl::Downloader,
};
use kithara_drm::KeyProcessorRegistry;
use kithara_platform::{CancelToken, sync::Arc, time::Duration};
use kithara_worker::Worker;
use struct_patch::Patch;
use url::Url;

use crate::{
    pools::{AppStore, AppWorker},
    theme::Palette,
};

/// App-owned snapshot of one DRM policy and its ordinary resolver registry.
#[derive(Clone, Debug, fieldwork::Fieldwork)]
#[non_exhaustive]
#[fieldwork(opt_in, get)]
pub struct AppDrm {
    policy: Arc<DomainKeyPolicy>,
    #[field(get)]
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

    /// Return resource headers selected by the same registered policy.
    #[must_use]
    pub fn resource_headers(&self, url: &Url) -> Option<kithara::net::Headers> {
        self.policy.resource_headers(url)
    }
}

/// Application configuration passed to the GUI frontend.
///
/// Shared owners and the downloader are mandatory; product knobs carry the
/// crate's own defaults, which the configuration document patches through
/// [`AppSettings`].
#[derive(Clone, Builder, Patch)]
#[builder(state_mod(vis = "pub"))]
#[patch(name = "AppSettings")]
#[patch(attribute(derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)))]
#[patch(attribute(serde(default, deny_unknown_fields)))]
#[patch(attribute(non_exhaustive))]
#[non_exhaustive]
pub struct AppConfig {
    /// App-owned DRM policy and its opaque key-request registry.
    #[patch(skip)]
    pub drm: AppDrm,
    /// App-wide shared asset store.
    #[patch(skip)]
    pub store: AppStore,
    /// Source beat-analysis tunables.
    #[builder(default)]
    #[patch(skip)]
    pub beat_analysis: BeatAnalysisConfig<PlaybackResamplerBackend>,
    /// Fixed source duration covered by one progressive analysis chunk.
    #[builder(default = NonZeroU32::new(16).unwrap_or(NonZeroU32::MIN))]
    pub analysis_chunk_seconds: NonZeroU32,
    /// One playback worker shared by every deck in this app session.
    #[patch(skip)]
    pub worker: AppWorker,
    /// Optional base runtime shared by playback, analysis, and app-owned
    /// background dispatchers. Production supplies one; focused consumers may
    /// let each domain worker own its standalone base.
    #[patch(skip)]
    pub base_worker: Option<Worker>,
    /// App master cancel. Single owner for the whole app subtree; the
    /// queue, player, stores, and UI listener all derive children from
    /// it (see `main.rs`). The chain flag reaches the playback worker and HLS
    /// coord lock-free `is_cancelled()` reads; every subsystem derives its
    /// own [`CancelToken::child`] from this consumer-top master.
    #[patch(skip)]
    pub shutdown: CancelToken,
    /// Shared HTTP downloader for every track.
    #[patch(skip)]
    pub downloader: Downloader,
    /// Color palette for the UI.
    #[builder(default)]
    #[patch(skip)]
    pub palette: Palette,
    /// HLS size-estimation probe strategy (see
    /// [`kithara::hls::SizeProbeMethod`]).
    #[builder(default = SizeProbeMethod::Head)]
    #[patch(skip)]
    pub size_probe_method: SizeProbeMethod,
    /// Log filter directives.
    #[builder(default)]
    pub log_directives: Vec<String>,
    /// Audio file URLs or paths to play.
    #[builder(default)]
    #[patch(skip)]
    pub tracks: Vec<String>,
    /// Accept invalid TLS certificates. Test servers only.
    #[builder(default = false)]
    #[patch(skip)]
    pub should_accept_invalid_certs: bool,
    /// Crossfade duration in seconds.
    #[builder(default = 5.0)]
    #[patch(skip)]
    pub crossfade_seconds: f32,
    /// Media duration the broadcast mix tap may run ahead of the packager by.
    /// The app allocates that ring, so it owns its depth: a longer lead rides
    /// out a longer packager stall and pays for it in the memory those
    /// interleaved samples occupy.
    #[builder(default = Duration::from_secs(2))]
    #[patch(attribute(serde(default, with = "humantime_serde::option")))]
    pub broadcast_tap_lead: Duration,
    /// Upper bound on waveform buckets (native = one per FFT window). Only
    /// caps very long tracks, to bound the cached blob.
    #[builder(default = 96_000)]
    pub waveform_max_buckets: usize,
    /// Band count of the EQ layout every deck's player graph is built with.
    #[builder(default = 3)]
    pub eq_bands: usize,
    /// Where this application reads its UI package from. What is found there
    /// is laid over the documents this build carries, so the interface can be
    /// changed without a rebuild. A path that does not exist means no package
    /// was laid out and the build's own documents draw; `None` means this
    /// configuration names no package at all.
    #[patch(skip)]
    pub ui_package: Option<PathBuf>,
}

impl fmt::Debug for AppConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppConfig")
            .field("drm", &self.drm)
            .field("palette", &self.palette)
            .field("log_directives", &self.log_directives)
            .field("tracks", &self.tracks)
            .field("worker", &self.worker)
            .field(
                "base_worker_cancelled",
                &self.base_worker.as_ref().map(Worker::is_cancelled),
            )
            .field(
                "should_accept_invalid_certs",
                &self.should_accept_invalid_certs,
            )
            .field("crossfade_seconds", &self.crossfade_seconds)
            .field("broadcast_tap_lead", &self.broadcast_tap_lead)
            .field("waveform_max_buckets", &self.waveform_max_buckets)
            .field("eq_bands", &self.eq_bands)
            .field("beat_analysis", &self.beat_analysis)
            .field("analysis_chunk_seconds", &self.analysis_chunk_seconds)
            .field("size_probe_method", &self.size_probe_method)
            .finish_non_exhaustive()
    }
}
