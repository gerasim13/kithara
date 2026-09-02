use bon::Builder;
use kithara_abr::AbrMode;
use kithara_assets::AssetStore;
use kithara_audio::{AudioDecoderConfig, AudioSettings};
use kithara_bufpool::HasPool;
use kithara_events::EventBus;
use kithara_file::FileSettings;
use kithara_hls::{HlsSettings, KeyOptions};
use kithara_net::Headers;
use kithara_platform::{CancelToken, sync::Arc};
use kithara_stream::dl::Downloader;
use kithara_warp::StretchControls;
use url::Url;

use super::{ResourceSrc, resampler::PlaybackResamplerBackend};
use crate::{EngineLoad, PlayWorker};

/// Resource-level knobs a configuration document can override, plus the
/// per-source settings the stream this resource builds carries. Extracted out
/// of [`ResourceConfig`] so a document reaches exactly these tunables and
/// never the per-call wiring (`store`, `bus`, `cancel`, `downloader`,
/// `worker`, `engine_load`, `stretch`, `decoder`) or the per-stream input
/// (`src`, `hint`, `hls_base_url`, `discriminator`, `headers`, `keys`,
/// `initial_abr_mode`) that stays on [`ResourceConfig`] itself.
///
/// [`HlsSettings`], [`FileSettings`] and [`AudioSettings`] are held whole
/// rather than re-declared field by field: a knob either crate adds then
/// reaches the built stream with no second declaration here to keep in step.
#[derive(Clone, Debug, Builder)]
#[builder(state_mod(vis = "pub"))]
#[non_exhaustive]
pub struct ResourceSettings {
    /// HLS streaming knobs, handed whole to the [`HlsConfig`] this resource
    /// builds. Not reachable under `resource.hls`: a configuration document's
    /// live spelling for these is its own top-level `hls:` section, which
    /// types into `HlsSettingsPatch` and is applied to this value directly. A
    /// second path to one value is what a configuration document exists to
    /// prevent.
    ///
    /// [`HlsConfig`]: kithara_hls::HlsConfig
    #[builder(default)]
    pub hls: HlsSettings,
    /// File streaming knobs, handed whole to the [`FileConfig`] this resource
    /// builds. Not reachable under `resource.file`, for the same reason
    /// [`Self::hls`] is not: the document's live spelling is its own top-level
    /// `file:` section.
    ///
    /// [`FileConfig`]: kithara_file::FileConfig
    #[builder(default)]
    pub file: FileSettings,
    /// Audio-pipeline knobs handed whole to the [`AudioConfig`] this resource
    /// builds. Not reachable under `resource.audio`, for the same reason
    /// [`Self::hls`] is not: the document's live spelling is its own top-level
    /// `audio:` section, which types into `AudioSettingsPatch` and is applied
    /// to this value directly.
    ///
    /// [`AudioConfig`]: kithara_audio::AudioConfig
    #[builder(default)]
    pub audio: AudioSettings,
    /// Requested peak-bitrate ceiling in bits per second, held for an ABR
    /// reader that does not exist yet. `resource/build.rs` forwards this to
    /// neither branch, so no value here changes variant selection today, and
    /// the one caller of [`ResourceConfig::preferred_peak_bitrate`] is a test
    /// asserting the value survives `Loader::build_config`. Not a document key
    /// for exactly that reason: a document knob the binary ignores is worse
    /// than no knob. Make it one when the ABR wiring lands.
    #[builder(default = 0.0)]
    pub preferred_peak_bitrate: f64,
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
    use std::{num::NonZeroUsize, path::Path};

    use kithara_assets::AssetStore;
    use kithara_audio::{
        AudioSettings, ConsumerWakeMode, DecoderResamplerSettings, ResamplerBackend,
        ResamplerOptions,
    };
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
            config.settings.audio.consumer_wake_mode,
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
                        .audio(
                            AudioSettings::builder()
                                .preload_chunks(NonZeroUsize::new(5).expect("BUG: 5 > 0"))
                                .build(),
                        )
                        .build(),
                )
                .build();
        assert!(config.bus.is_some());
        assert_eq!(config.hint.as_deref(), Some("mp3"));
        assert_eq!(config.discriminator.as_deref(), Some("test"));
        assert_eq!(config.settings.audio.preload_chunks.get(), 5);
    }

    #[kithara::test]
    fn config_bitrate_fields_default_zero() {
        let config = test_config("https://example.com/live.m3u8").unwrap();
        assert!((config.settings.preferred_peak_bitrate - 0.0).abs() < f64::EPSILON);
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

        assert_eq!(settings.audio.preload_chunks.get(), 3);
        assert!((settings.preferred_peak_bitrate - 0.0).abs() < f64::EPSILON);
        assert_eq!(
            settings.audio.consumer_wake_mode,
            ConsumerWakeMode::RealtimeDeferred
        );
        assert!(!settings.audio.block_on_underrun);
        assert!(settings.audio.host_sample_rate.is_none());
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
