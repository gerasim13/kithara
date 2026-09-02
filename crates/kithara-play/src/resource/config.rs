use std::num::{NonZeroU32, NonZeroUsize};

use bon::Builder;
use kithara_abr::AbrMode;
use kithara_assets::AssetStore;
use kithara_audio::{AudioDecoderConfig, ConsumerWakeMode};
use kithara_bufpool::HasPool;
use kithara_events::EventBus;
use kithara_file::FileSettings;
use kithara_hls::{HlsSettings, KeyOptions};
use kithara_net::Headers;
use kithara_platform::{CancelToken, sync::Arc};
use kithara_stream::dl::Downloader;
use kithara_warp::StretchControls;
use struct_patch::Patch;
use url::Url;

use super::{ResourceSrc, resampler::PlaybackResamplerBackend};
use crate::{EngineLoad, PlayWorker};

/// Default number of preload chunks.
const DEFAULT_PRELOAD_CHUNKS: NonZeroUsize = NonZeroUsize::new(3).unwrap();

/// Resource-level knobs a configuration document can override, plus the
/// per-source settings the stream this resource builds carries. Extracted out
/// of [`ResourceConfig`] so a document reaches exactly these tunables and
/// never the per-call wiring (`store`, `bus`, `cancel`, `downloader`,
/// `worker`, `engine_load`, `stretch`, `decoder`) or the per-stream input
/// (`src`, `hint`, `hls_base_url`, `discriminator`, `headers`, `keys`,
/// `initial_abr_mode`) that stays on [`ResourceConfig`] itself.
///
/// [`HlsSettings`] and [`FileSettings`] are held whole rather than re-declared
/// field by field: a knob either crate adds then reaches the built stream with
/// no second declaration here to keep in step.
#[derive(Clone, Debug, Builder, Patch)]
#[builder(state_mod(vis = "pub"))]
#[patch(name = "ResourceSettingsPatch")]
#[patch(attribute(derive(Clone, Debug, Default, serde::Deserialize)))]
#[patch(attribute(serde(default, deny_unknown_fields)))]
#[patch(attribute(non_exhaustive))]
#[non_exhaustive]
pub struct ResourceSettings {
    /// HLS streaming knobs, handed whole to the [`HlsConfig`] this resource
    /// builds. Not reachable under `resource.hls`: a configuration document's
    /// live spelling for these is its own top-level `hls:` section, which
    /// types into `HlsSettingsPatch` and is applied to this value directly. A
    /// second path to one value is what a configuration document exists to
    /// prevent, so this field carries `#[patch(skip)]`.
    ///
    /// [`HlsConfig`]: kithara_hls::HlsConfig
    #[builder(default)]
    #[patch(skip)]
    pub hls: HlsSettings,
    /// File streaming knobs, handed whole to the [`FileConfig`] this resource
    /// builds. Not reachable under `resource.file`, for the same reason
    /// [`Self::hls`] is not: the document's live spelling is its own top-level
    /// `file:` section.
    ///
    /// [`FileConfig`]: kithara_file::FileConfig
    #[builder(default)]
    #[patch(skip)]
    pub file: FileSettings,
    /// Number of chunks to buffer before signaling preload readiness.
    #[builder(default = DEFAULT_PRELOAD_CHUNKS)]
    pub preload_chunks: NonZeroUsize,
    /// Maximum peak bitrate in bits per second for ABR variant selection.
    /// `0.0` leaves the choice to the ABR controller.
    #[builder(default = 0.0)]
    pub preferred_peak_bitrate: f64,
    /// Audio-consumer wake capability for this resource's reader. The default
    /// is safe for a consumer on the real-time render callback.
    /// `PlayerImpl::prepare_config` always overwrites it with the session
    /// policy, so a player-managed resource cannot carry a second source of
    /// that policy. A direct reader off the real-time thread opts into
    /// [`ConsumerWakeMode::ImmediateOffRt`] itself for immediate worker wakes
    /// and inline reader-event delivery. Not a document key: declaring
    /// `ImmediateOffRt` on a player-bound resource would make its reads
    /// publish inline on the render callback, and every player-managed
    /// resource has the value overwritten anyway.
    #[builder(default)]
    #[patch(skip)]
    pub consumer_wake_mode: ConsumerWakeMode,
    /// Make audio-thread reads block on a producer-ring underrun instead of
    /// zero-filling. `PlayerImpl::prepare_config` copies the player's policy
    /// here; a direct reader off the real-time thread may opt in itself. Not a
    /// document key: the shipped binary is a real-time host whose audio
    /// callback can never block, and only the offline test harness sets this,
    /// from Rust.
    #[builder(default)]
    #[patch(skip)]
    pub block_on_underrun: bool,
    /// Target sample rate of the audio host (for resampling). Not a document
    /// key: this is the rate the audio host actually opened, written at
    /// runtime by [`ResourceConfig::set_host_sample_rate`] from the render
    /// thread's `SetSampleRate` command, so a document value would be
    /// overwritten by the first host that disagrees with it.
    #[patch(skip)]
    pub host_sample_rate: Option<NonZeroU32>,
}

impl Default for ResourceSettings {
    fn default() -> Self {
        Self::builder().build()
    }
}

/// Unified configuration for opening an audio resource.
#[derive(Builder)]
#[builder(on(String, into), start_fn = for_src)]
#[non_exhaustive]
pub struct ResourceConfig<S, B: Default = PlaybackResamplerBackend>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    /// Audio resource source (URL or local path).
    #[builder(start_fn)]
    pub(crate) src: ResourceSrc,
    /// Initial ABR mode passed to the HLS stream.
    #[builder(default)]
    pub(crate) initial_abr_mode: AbrMode,
    /// Shared asset store used by playback and derived resources.
    pub(crate) store: AssetStore<S>,
    /// Decoder construction settings: backend selection, gapless mode, and
    /// decoder-side resampling.
    #[builder(default)]
    pub(crate) decoder: AudioDecoderConfig<B>,
    /// Encryption key handling configuration.
    #[builder(default)]
    pub(crate) keys: KeyOptions,
    /// Resource-level knobs plus the HLS and file settings the built stream
    /// carries. See [`ResourceSettings`].
    #[builder(default)]
    pub(crate) settings: ResourceSettings,
    /// Unified event bus for streaming, decode, and audio events.
    #[builder(name = events)]
    pub(crate) bus: Option<EventBus>,
    /// Per-track parent cancel. The atomic flag reaches the HLS coord's
    /// lock-free `is_cancelled()` read; downloader / file / decode paths derive
    /// children via [`CancelToken::child`]. `None` lets each subsystem own a
    /// standalone scope (see [`CancelScope::new`](kithara_platform::CancelScope)).
    pub(crate) cancel: Option<CancelToken>,
    /// Optional cache discriminator mixed into the asset root.
    pub(crate) discriminator: Option<String>,
    /// Shared downloader instance.
    pub(crate) downloader: Option<Downloader>,
    /// Shared live audio-engine cost meter (decode + effects).
    pub(crate) engine_load: Option<Arc<EngineLoad>>,
    /// Additional HTTP headers to include in all network requests.
    pub(crate) headers: Option<Headers>,
    /// Optional format hint (file extension like "mp3", "wav"). Per-call input
    /// read twice: the file branch maps it into [`FileSettings::extension`],
    /// and both branches pass it to the decoder as a format hint.
    pub(crate) hint: Option<String>,
    /// Base URL for resolving relative HLS playlist/segment URLs.
    pub(crate) hls_base_url: Option<Url>,
    /// Live time-stretch controls shared with the resident Warp chain.
    #[builder(default = StretchControls::new(1.0))]
    pub(crate) stretch: Arc<StretchControls>,
    /// Explicit playback worker. Player preparation fills this field; direct
    /// Resource callers must configure it themselves.
    pub(crate) worker: Option<PlayWorker<S>>,
}

impl<S, B> Clone for ResourceConfig<S, B>
where
    B: Clone + Default,
    S: HasPool<u8> + Send + Sync + 'static,
{
    fn clone(&self) -> Self {
        Self {
            src: self.src.clone(),
            initial_abr_mode: self.initial_abr_mode,
            store: self.store.clone(),
            decoder: self.decoder.clone(),
            keys: self.keys.clone(),
            settings: self.settings.clone(),
            bus: self.bus.clone(),
            cancel: self.cancel.clone(),
            discriminator: self.discriminator.clone(),
            downloader: self.downloader.clone(),
            engine_load: self.engine_load.clone(),
            headers: self.headers.clone(),
            hint: self.hint.clone(),
            hls_base_url: self.hls_base_url.clone(),
            stretch: Arc::clone(&self.stretch),
            worker: self.worker.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use kithara_assets::AssetStore;
    use kithara_audio::{DecoderResamplerSettings, ResamplerBackend, ResamplerOptions};
    use kithara_decode::DecodeError;
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        PlayWorkerConfig,
        test_pools::{TestPools, pools},
    };

    fn store() -> AssetStore<TestPools> {
        AssetStore::builder(pools()).build()
    }

    fn valid_src(input: &str) -> ResourceSrc {
        ResourceSrc::parse(input).expect("valid test source")
    }

    fn test_config<S: AsRef<str>>(input: S) -> Result<ResourceConfig<TestPools>, DecodeError> {
        Ok(ResourceConfig::for_src(ResourceSrc::parse(input)?)
            .store(store())
            .build())
    }

    #[kithara::test]
    fn a_config_that_never_passed_a_player_defaults_to_realtime_deferred() {
        let config = test_config("https://example.com/track.mp3").expect("valid config");
        assert_eq!(
            config.settings.consumer_wake_mode,
            ConsumerWakeMode::RealtimeDeferred
        );
    }

    fn worker() -> PlayWorker<TestPools> {
        PlayWorker::new(PlayWorkerConfig::builder(pools()).build())
    }

    #[kithara::test]
    fn config_source_parsing_url() {
        let config = test_config("https://example.com/song.mp3").unwrap();
        assert!(matches!(&config.src, ResourceSrc::Url(url) if url.scheme() == "https"));
    }

    #[kithara::test]
    fn config_file_url_derives_extension_hint_from_last_path_segment() {
        let worker = worker();
        let config = test_config("https://example.com/audio/get-mp3/song.MP3?sign=test")
            .unwrap()
            .build_file_config(&worker, None);

        assert_eq!(config.hint(), Some("mp3"));
    }

    #[kithara::test]
    fn config_file_url_without_extension_does_not_derive_hint() {
        let worker = worker();
        let config = test_config("https://example.com/get-mp3/42?sign=test")
            .unwrap()
            .build_file_config(&worker, None);

        assert_eq!(config.hint(), None);
    }

    #[kithara::test(native)]
    #[case("/tmp/song.mp3", "/tmp/song.mp3")]
    #[case("file:///tmp/song.mp3", "/tmp/song.mp3")]
    fn config_source_parsing_file_path(#[case] input: &str, #[case] expected: &str) {
        let config = test_config(input).unwrap();
        assert!(matches!(
            &config.src,
            ResourceSrc::Path(path) if path == Path::new(expected)
        ));
    }

    #[kithara::test]
    #[case("relative/path.mp3")]
    fn config_source_parsing_error(#[case] input: &str) {
        assert!(test_config(input).is_err());
    }

    #[kithara::test]
    #[case(false)]
    #[case(true)]
    fn config_bus_presence(#[case] with_events: bool) {
        let config: ResourceConfig<TestPools> =
            ResourceConfig::for_src(valid_src("https://example.com/song.mp3"))
                .store(store())
                .maybe_events(with_events.then(|| EventBus::new(32)))
                .build();
        assert_eq!(config.bus.is_some(), with_events);
    }

    #[kithara::test]
    fn config_bus_propagates_to_file_config() {
        let worker = worker();
        let config: ResourceConfig<TestPools> =
            ResourceConfig::for_src(valid_src("https://example.com/song.mp3"))
                .store(store())
                .events(EventBus::new(32))
                .build();
        let audio_config = config.build_file_config(&worker, None);
        assert!(audio_config.stream().bus.is_some());
    }

    #[kithara::test]
    fn config_bus_propagates_to_hls_config() {
        let worker = worker();
        let config: ResourceConfig<TestPools> =
            ResourceConfig::for_src(valid_src("https://example.com/live.m3u8"))
                .store(store())
                .events(EventBus::new(32))
                .build();
        let audio_config = config.build_hls_config(&worker, None).unwrap();
        assert!(audio_config.stream().bus.is_some());
    }

    #[kithara::test]
    fn config_resampler_options_propagate_to_file_config() {
        let worker = worker();
        let decoder = AudioDecoderConfig::builder()
            .resampler(
                DecoderResamplerSettings::builder()
                    .backend(PlaybackResamplerBackend::default())
                    .options(ResamplerOptions::builder().chunk_size(2_048).build())
                    .build(),
            )
            .build();
        let config: ResourceConfig<TestPools> =
            ResourceConfig::for_src(valid_src("https://example.com/song.mp3"))
                .store(store())
                .decoder(decoder)
                .build();
        let audio_config = config.build_file_config(&worker, None);

        assert_eq!(
            audio_config
                .decoder()
                .resampler()
                .expect("resampler config")
                .options()
                .chunk_size,
            2_048
        );
    }

    #[kithara::test]
    fn config_explicit_resampler_backend_propagates_to_hls_config() {
        let worker = worker();
        let decoder = AudioDecoderConfig::builder()
            .resampler(
                DecoderResamplerSettings::builder()
                    .backend(PlaybackResamplerBackend::default())
                    .build(),
            )
            .build();
        let config: ResourceConfig<TestPools> =
            ResourceConfig::for_src(valid_src("https://example.com/live.m3u8"))
                .store(store())
                .decoder(decoder)
                .build();
        let audio_config = config.build_hls_config(&worker, None).unwrap();

        assert_eq!(
            audio_config
                .decoder()
                .resampler()
                .expect("resampler config")
                .backend()
                .name(),
            PlaybackResamplerBackend::default().name()
        );
    }

    #[kithara::test]
    fn config_with_headers() {
        let mut headers = Headers::default();
        headers.insert("Authorization", "Bearer test");
        let config: ResourceConfig<TestPools> =
            ResourceConfig::for_src(valid_src("https://example.com/song.mp3"))
                .store(store())
                .headers(headers)
                .build();

        assert!(config.headers.is_some());
        assert_eq!(
            config.headers.as_ref().and_then(|h| h.get("Authorization")),
            Some("Bearer test")
        );
    }

    #[kithara::test]
    fn config_builder_chain() {
        let config: ResourceConfig<TestPools> =
            ResourceConfig::for_src(valid_src("https://example.com/song.mp3"))
                .store(store())
                .events(EventBus::new(32))
                .hint("mp3")
                .discriminator("test")
                .settings(
                    ResourceSettings::builder()
                        .preload_chunks(NonZeroUsize::new(5).expect("BUG: 5 > 0"))
                        .build(),
                )
                .build();
        assert!(config.bus.is_some());
        assert_eq!(config.hint.as_deref(), Some("mp3"));
        assert_eq!(config.discriminator.as_deref(), Some("test"));
        assert_eq!(config.settings.preload_chunks.get(), 5);
    }

    #[kithara::test]
    fn config_bitrate_fields_default_zero() {
        let config = test_config("https://example.com/live.m3u8").unwrap();
        assert!((config.settings.preferred_peak_bitrate - 0.0).abs() < f64::EPSILON);
    }

    #[kithara::test]
    fn config_bitrate_propagates_to_hls_abr() {
        let worker = worker();
        let config: ResourceConfig<TestPools> =
            ResourceConfig::for_src(valid_src("https://example.com/live.m3u8"))
                .store(store())
                .settings(
                    ResourceSettings::builder()
                        .preferred_peak_bitrate(512_000.0)
                        .build(),
                )
                .build();
        let _audio_config = config.build_hls_config(&worker, None).unwrap();
    }

    #[kithara::test]
    fn config_worker_default_none() {
        let config = test_config("https://example.com/song.mp3").unwrap();
        assert!(config.worker.is_none());
    }

    #[kithara::test]
    fn config_stretch_defaults_to_unity() {
        let config = test_config("https://example.com/song.mp3").unwrap();
        assert!((config.stretch.speed() - 1.0).abs() < f32::EPSILON);
    }

    #[kithara::test]
    fn config_with_worker_sets_field() {
        let worker = worker();
        let config: ResourceConfig<TestPools> =
            ResourceConfig::for_src(valid_src("https://example.com/song.mp3"))
                .store(store())
                .worker(worker.clone())
                .build();
        let configured = config.worker.as_ref().expect("worker must be configured");
        assert!(std::ptr::eq(configured.pools(), worker.pools()));
    }

    #[kithara::test]
    fn file_hint_none_for_url_without_extension() {
        let worker = worker();
        let config = test_config("https://cdn-edge.zvq.me/track/streamhq?id=125475417").unwrap();
        let audio_config = config.build_file_config(&worker, None);
        assert_eq!(
            audio_config.hint(),
            None,
            "URL without file extension must produce hint=None"
        );
    }

    #[kithara::test]
    fn defaults_match_the_documented_values() {
        let settings = ResourceSettings::default();

        assert_eq!(settings.preload_chunks.get(), 3);
        assert!((settings.preferred_peak_bitrate - 0.0).abs() < f64::EPSILON);
        assert_eq!(
            settings.consumer_wake_mode,
            ConsumerWakeMode::RealtimeDeferred
        );
        assert!(!settings.block_on_underrun);
        assert!(settings.host_sample_rate.is_none());
        assert_eq!(
            settings.hls.download_batch_size, 3,
            "the held HLS settings carry kithara-hls's own defaults"
        );
        assert_eq!(
            settings.file.reader_event_capacity, 256,
            "the held file settings carry kithara-file's own defaults"
        );
    }

    #[kithara::test]
    #[case("https://example.com/song.mp3", Some("mp3"))]
    #[case("https://example.com/audio.flac", Some("flac"))]
    #[case("https://example.com/track/stream", None)]
    #[case("https://example.com/track/streamhq?id=123", None)]
    #[case("https://example.com/audio", None)]
    fn file_hint_from_url_extension(#[case] url: &str, #[case] expected: Option<&str>) {
        let worker = worker();
        let config = test_config(url).unwrap();
        let audio_config = config.build_file_config(&worker, None);
        assert_eq!(
            audio_config.hint(),
            expected,
            "hint mismatch for URL: {url}"
        );
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod document_tests {
    use kithara_test_utils::kithara;
    use struct_patch::Patch as _;

    use super::{ResourceSettings, ResourceSettingsPatch};

    /// `deny_unknown_fields` arrives through `#[patch(attribute(...))]`, which
    /// emits its token stream verbatim. A typo there would generate a patch
    /// that accepts anything, and neither the compiler nor clippy would say a
    /// word -- only a bogus key proves the attribute survived generation.
    /// `chunk_budget` shares no prefix with either declared field, so the
    /// assertion cannot pass off serde's list of valid names.
    #[kithara::test(native, flash(false))]
    fn an_unknown_field_is_rejected_and_named() {
        let error = serde_yaml_ng::from_str::<ResourceSettingsPatch>("chunk_budget: 8\n")
            .expect_err("a typo must not be silently ignored");

        assert!(error.to_string().contains("chunk_budget"), "{error}");
    }

    #[kithara::test(native, flash(false))]
    fn a_patch_writes_only_the_preload_count_it_names() {
        let patch: ResourceSettingsPatch =
            serde_yaml_ng::from_str("preload_chunks: 8\n").expect("the document types");
        let mut settings = ResourceSettings::default();
        // Seeded off the defaults (0.0 and 3) so a whole-struct `apply` that
        // resets every unnamed field to `Default::default()` cannot pass these
        // assertions by coincidence.
        settings.preferred_peak_bitrate = 320_000.0;
        settings.hls.download_batch_size = 6;

        settings.apply(patch);

        assert_eq!(settings.preload_chunks.get(), 8);
        assert!(
            (settings.preferred_peak_bitrate - 320_000.0).abs() < f64::EPSILON,
            "a silent field must keep its seeded value, not reset to default"
        );
        assert_eq!(
            settings.hls.download_batch_size, 6,
            "the held HLS settings are written by their own section, not by this patch"
        );
    }

    /// `hls` is a real field on `ResourceSettings` but must not be reachable
    /// under `resource.hls`: the document's live spelling for those knobs is
    /// its own top-level `hls:` section (see the field's doc comment).
    #[kithara::test(native, flash(false))]
    fn the_held_hls_settings_are_not_a_document_key() {
        let error =
            serde_yaml_ng::from_str::<ResourceSettingsPatch>("hls:\n  download_batch_size: 6\n")
                .expect_err("the top-level `hls:` section is the only spelling");

        assert!(error.to_string().contains("hls"), "{error}");
    }

    /// `file` is a real field on `ResourceSettings` but must not be reachable
    /// under `resource.file`, for the same reason `hls` is not.
    #[kithara::test(native, flash(false))]
    fn the_held_file_settings_are_not_a_document_key() {
        let error = serde_yaml_ng::from_str::<ResourceSettingsPatch>(
            "file:\n  reader_event_capacity: 512\n",
        )
        .expect_err("the top-level `file:` section is the only spelling");

        assert!(error.to_string().contains("file"), "{error}");
    }

    /// `consumer_wake_mode` is overwritten for every player-managed resource,
    /// and declaring `ImmediateOffRt` on a player-bound one would make its
    /// reads publish inline on the render callback.
    #[kithara::test(native, flash(false))]
    fn the_realtime_unsafe_wake_mode_is_not_a_document_key() {
        let error = serde_yaml_ng::from_str::<ResourceSettingsPatch>(
            "consumer_wake_mode: immediate_off_rt\n",
        )
        .expect_err(
            "a capability that moves reads onto the render callback is not document-settable",
        );

        assert!(error.to_string().contains("consumer_wake_mode"), "{error}");
    }

    /// `block_on_underrun` is overwritten for every player-managed resource
    /// and can park the real-time audio callback.
    #[kithara::test(native, flash(false))]
    fn the_realtime_unsafe_block_on_underrun_field_is_not_a_document_key() {
        let error = serde_yaml_ng::from_str::<ResourceSettingsPatch>("block_on_underrun: true\n")
            .expect_err("a field that can park the audio callback must not be document-settable");

        assert!(error.to_string().contains("block_on_underrun"), "{error}");
    }

    /// `host_sample_rate` is the rate the audio host actually opened, written
    /// at runtime from the render thread's `SetSampleRate` command.
    #[kithara::test(native, flash(false))]
    fn the_runtime_owned_host_sample_rate_is_not_a_document_key() {
        let error = serde_yaml_ng::from_str::<ResourceSettingsPatch>("host_sample_rate: 48000\n")
            .expect_err("the audio host owns its own rate");

        assert!(error.to_string().contains("host_sample_rate"), "{error}");
    }
}
