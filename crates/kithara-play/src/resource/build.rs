use kithara_audio::{AudioConfig, AudioObserver, ResamplerBackend};
use kithara_bufpool::HasPool;
use kithara_decode::DecodeError;
use kithara_file::{FileConfig, FileSrc};
use kithara_hls::HlsConfig;
use kithara_net::{HttpClient, NetOptions};
use kithara_platform::CancelScope;
use kithara_stream::dl::{Downloader, DownloaderConfig};
use url::Url;

use super::{ResourceConfig, ResourceSrc};
use crate::PlayWorker;

fn derive_remote_file_hint(url: &Url) -> Option<String> {
    url.path_segments()
        .and_then(|mut segments| segments.next_back())
        .and_then(derive_extension_hint)
}

fn derive_extension_hint(segment: &str) -> Option<String> {
    let (_, extension) = segment.rsplit_once('.')?;
    if extension.is_empty() || !extension.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        return None;
    }
    Some(extension.to_lowercase())
}

impl<S, B> ResourceConfig<S, B>
where
    B: Default + ResamplerBackend,
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    /// Build an `AudioConfig<File<S>>` from this resource configuration.
    pub(crate) fn build_file_config(
        self,
        worker: &PlayWorker<S>,
        observer: Option<Box<dyn AudioObserver>>,
    ) -> AudioConfig<kithara_file::File<S>, B> {
        let pools = worker.pools().clone();
        let (file_src, derived_hint) = match self.src {
            ResourceSrc::Url(ref url) => {
                (FileSrc::Remote(url.clone()), derive_remote_file_hint(url))
            }
            ResourceSrc::Path(ref path) => (
                FileSrc::Local(path.clone()),
                path.extension()
                    .and_then(|e| e.to_str())
                    .map(str::to_lowercase),
            ),
        };
        let extension = self
            .hint
            .clone()
            .or(derived_hint)
            .or_else(|| self.settings.file.extension.clone());
        let downloader = self.downloader.clone().unwrap_or_else(|| {
            let dl_cancel = CancelScope::new(self.cancel.clone()).token();
            let client = HttpClient::new(NetOptions::default(), pools.clone(), dl_cancel.child());
            Downloader::new(
                DownloaderConfig::for_client(client)
                    .cancel(dl_cancel)
                    .build(),
            )
        });
        let mut settings = self.settings.file.clone();
        settings.extension = extension.clone();
        let file_config = FileConfig::for_src(file_src)
            .store(self.store.clone())
            .downloader(downloader)
            .maybe_headers(self.headers.clone())
            .maybe_discriminator(self.discriminator.clone())
            .pools(pools)
            .maybe_events(self.bus.clone())
            .maybe_cancel(self.cancel.clone())
            .settings(settings)
            .build();
        AudioConfig::<kithara_file::File<S>, B>::for_stream(file_config)
            .maybe_cancel(self.cancel.clone())
            .maybe_hint(extension)
            .maybe_observer(observer)
            .settings(self.settings.audio.clone())
            .decoder(self.decoder)
            .build()
    }

    /// Build an `AudioConfig<Hls<S>>` from this resource configuration.
    pub(crate) fn build_hls_config(
        self,
        worker: &PlayWorker<S>,
        observer: Option<Box<dyn AudioObserver>>,
    ) -> Result<AudioConfig<kithara_hls::Hls<S>, B>, DecodeError> {
        let pools = worker.pools().clone();
        let url = match self.src {
            ResourceSrc::Url(ref url) => url.clone(),
            ResourceSrc::Path(_) => {
                return Err(DecodeError::InvalidData {
                    detail: "HLS requires a URL, got a local path",
                });
            }
        };
        let hls_config = HlsConfig::for_url(url)
            .store(self.store.clone())
            .keys(self.keys)
            .maybe_downloader(self.downloader)
            .initial_abr_mode(self.initial_abr_mode)
            .maybe_headers(self.headers)
            .maybe_discriminator(self.discriminator)
            .maybe_base_url(self.hls_base_url)
            .pools(pools)
            .maybe_events(self.bus.clone())
            .maybe_cancel(self.cancel.clone())
            .settings(self.settings.hls.clone())
            .build();
        Ok(
            AudioConfig::<kithara_hls::Hls<S>, B>::for_stream(hls_config)
                .maybe_cancel(self.cancel.clone())
                .maybe_hint(self.hint)
                .maybe_observer(observer)
                .settings(self.settings.audio.clone())
                .decoder(self.decoder)
                .build(),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;

    use kithara_assets::AssetStore;
    use kithara_audio::AudioSettings;
    use kithara_test_utils::kithara;

    use crate::{
        PlayWorker, PlayWorkerConfig,
        resource::{ResourceConfig, ResourceSettings, ResourceSrc},
        test_pools::{TestPools, pools},
    };

    fn worker() -> PlayWorker<TestPools> {
        PlayWorker::new(PlayWorkerConfig::builder(pools()).build())
    }

    fn config(source: &str, settings: ResourceSettings) -> ResourceConfig<TestPools> {
        ResourceConfig::for_src(ResourceSrc::parse(source).expect("valid test source"))
            .store(AssetStore::builder(pools()).build())
            .settings(settings)
            .build()
    }

    /// `download_batch_size` reached the built stream through no path while
    /// `ResourceConfig` re-declared HLS knobs one by one: it never mirrored
    /// this one, so a caller going through `PlayerConfig` could not set it at
    /// all. Passing `HlsSettings` whole closes that gap for every knob the
    /// crate declares, now and later.
    #[kithara::test]
    fn an_hls_knob_the_resource_never_declared_reaches_the_built_config() {
        let mut settings = ResourceSettings::default();
        settings.hls.download_batch_size = 6;

        let built = config("https://example.com/live.m3u8", settings)
            .build_hls_config(&worker(), None)
            .expect("valid HLS config");

        assert_eq!(built.stream().settings.download_batch_size, 6);
    }

    /// The same for the file branch: `reader_event_capacity` is a
    /// `FileSettings` knob `ResourceConfig` never mirrored.
    #[kithara::test]
    fn a_file_knob_the_resource_never_declared_reaches_the_built_config() {
        let mut settings = ResourceSettings::default();
        settings.file.reader_event_capacity = 512;

        let built =
            config("https://example.com/song.mp3", settings).build_file_config(&worker(), None);

        assert_eq!(built.stream().settings.reader_event_capacity, 512);
    }

    /// The per-call `hint` still lands as the file source's extension: it is
    /// per-call input, read by the decoder as well, so it stays on
    /// `ResourceConfig` and is mapped into `FileSettings::extension` once here.
    #[kithara::test]
    fn the_per_call_hint_becomes_the_file_extension() {
        let built = ResourceConfig::<TestPools>::for_src(
            ResourceSrc::parse("https://example.com/track/stream").expect("valid test source"),
        )
        .store(AssetStore::builder(pools()).build())
        .hint("flac")
        .build()
        .build_file_config(&worker(), None);

        assert_eq!(built.stream().settings.extension.as_deref(), Some("flac"));
        assert_eq!(built.hint(), Some("flac"));
    }

    /// A document-named `extension` backs the per-call hint rather than being
    /// dropped: nothing more specific names one for a URL that carries no
    /// extension and no caller hint.
    #[kithara::test]
    fn a_settings_extension_stands_when_nothing_more_specific_names_one() {
        let mut settings = ResourceSettings::default();
        settings.file.extension = Some("wav".to_owned());

        let built =
            config("https://example.com/track/stream", settings).build_file_config(&worker(), None);

        assert_eq!(built.stream().settings.extension.as_deref(), Some("wav"));
        assert_eq!(built.hint(), Some("wav"));
    }

    fn preload_chunks(count: usize) -> ResourceSettings {
        ResourceSettings::builder()
            .audio(
                AudioSettings::builder()
                    .preload_chunks(NonZeroUsize::new(count).expect("a preload count above zero"))
                    .build(),
            )
            .build()
    }

    /// `preload_chunks` is the one `ResourceSettings` field that is both a
    /// document key and read in production, so the value a document names has
    /// to survive the whole way to the built HLS pipeline.
    #[kithara::test]
    fn the_document_preload_count_reaches_the_built_hls_config() {
        let built = config("https://example.com/live.m3u8", preload_chunks(9))
            .build_hls_config(&worker(), None)
            .expect("valid HLS config");

        assert_eq!(built.preload_chunks().get(), 9);
    }

    /// The file branch reads the same field from the same place.
    #[kithara::test]
    fn the_document_preload_count_reaches_the_built_file_config() {
        let built = config("https://example.com/song.mp3", preload_chunks(9))
            .build_file_config(&worker(), None);

        assert_eq!(built.preload_chunks().get(), 9);
    }

    /// `audio_buffer_chunks` reached the built `AudioConfig` through no path
    /// while `ResourceConfig` forwarded individual audio fields one by one:
    /// it never mirrored this one. Handing `AudioSettings` whole closes that
    /// gap, the same collapse `hls` and `file` already went through.
    #[kithara::test]
    fn an_audio_knob_the_resource_never_declared_reaches_the_built_config() {
        let mut settings = ResourceSettings::default();
        settings.audio.audio_buffer_chunks = 24;

        let built = config("https://example.com/live.m3u8", settings)
            .build_hls_config(&worker(), None)
            .expect("valid HLS config");

        assert_eq!(built.audio_buffer_chunks(), 24);
    }
}
