use std::num::NonZeroUsize;

use kithara_audio::{AudioConfig, ResamplerBackend};
use kithara_config::Config;
use kithara_dsp::param::SmootherConfig;
use kithara_effects::AudioEffect;
use kithara_platform::sync::Arc;
use kithara_stream::StreamType;
use kithara_warp::WarpConfig;

use super::EngineLoad;

/// Play-owned configuration for one resident Warp/audio producer lane.
#[derive(Config, fieldwork::Fieldwork)]
#[config(construction, builder(start_fn = for_audio))]
#[fieldwork(opt_in, get)]
#[non_exhaustive]
pub struct TrackConfig<T, B>
where
    T: StreamType,
    B: ResamplerBackend,
{
    /// Source-only decoder configuration.
    #[config(
        skip = "transferred to the play-owned audio producer",
        builder(start_fn)
    )]
    #[field(get)]
    pub(crate) audio: AudioConfig<T, B>,
    /// Final rendered chunks required before initial or segment readiness.
    #[config(value, builder(default = NonZeroUsize::MIN))]
    #[field(get, copy)]
    pub(crate) preload_chunks: NonZeroUsize,
    /// Lane-owned Jump ramp, using the deck's frame rounding.
    #[config(value, builder(default = crate::consts::DEFAULT_DECLICK))]
    #[field(get, copy)]
    pub(crate) declick: SmootherConfig,
    /// Chunk capacity of each forward and reverse PCM ring.
    #[config(value, builder(default = crate::consts::CAPACITY))]
    #[field(get, copy)]
    pub(crate) audio_buffer_chunks: NonZeroUsize,
    /// Make offline reads of playing slots wait for PCM or lane closure instead of underrunning.
    #[config(
        skip = "blocking reads are an explicit off-real-time choice",
        builder(default)
    )]
    #[field(get, copy)]
    pub(crate) block_on_underrun: bool,
    /// Optional live cost meter for this play-owned producer lane.
    #[field(get)]
    #[config(skip = "transferred to the producer cost meter")]
    pub(crate) engine_load: Option<Arc<EngineLoad>>,
    /// Playback effects after the resident Warp stage.
    #[config(skip = "transferred to the effect chain", builder(default))]
    #[field(get)]
    pub(crate) effects: Vec<Box<dyn AudioEffect>>,
    /// Resident Warp resources and live temporal controls.
    #[config(skip = "transferred to the warp lane", builder(default = WarpConfig::builder().build()))]
    #[field(get)]
    pub(crate) warp: WarpConfig,
}

impl<T: StreamType, B: ResamplerBackend> TrackConfig<T, B> {
    /// Output frames to subtract from a Jump's intended landing frame.
    #[must_use]
    pub fn declick_frames(&self, rate: std::num::NonZeroU32) -> kithara_signal::FrameCount {
        crate::rt::declick_frame_count(self.declick, rate)
    }
}

impl<T, B> From<AudioConfig<T, B>> for TrackConfig<T, B>
where
    T: StreamType,
    B: ResamplerBackend,
{
    fn from(audio: AudioConfig<T, B>) -> Self {
        Self::for_audio(audio).build()
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use kithara_assets::{AssetStore, StorageBackend};
    use kithara_audio::{AudioConfig, NoResamplerBackend};
    use kithara_file::{File, FileConfig, FileSrc};
    use kithara_test_utils::kithara;

    use super::TrackConfig;
    use crate::test_pools::{TestPools, pools};

    fn file_config() -> FileConfig<TestPools> {
        let pools = pools();
        FileConfig::for_src(FileSrc::Local(
            std::env::temp_dir().join("kithara-audio-config.wav"),
        ))
        .store(
            AssetStore::builder(pools.clone())
                .backend(StorageBackend::Memory)
                .build(),
        )
        .pools(pools)
        .build()
    }

    #[kithara::test]
    fn audio_config_keeps_the_native_ring_and_preload_defaults() {
        let audio =
            AudioConfig::<File<TestPools>, NoResamplerBackend>::for_stream(file_config()).build();
        let config = TrackConfig::for_audio(audio).build();

        assert_eq!(config.audio_buffer_chunks().get(), 10);
        assert_eq!(config.preload_chunks().get(), 3);
    }

    #[kithara::test]
    fn the_document_names_both_live_keys() {
        let audio =
            AudioConfig::<File<TestPools>, NoResamplerBackend>::for_stream(file_config()).build();
        let config = TrackConfig::for_audio(audio)
            .preload_chunks(std::num::NonZeroUsize::new(8).expect("preload"))
            .audio_buffer_chunks(std::num::NonZeroUsize::new(20).expect("capacity"))
            .build();
        assert_eq!(
            config.preload_chunks(),
            std::num::NonZeroUsize::new(8).expect("preload")
        );
        assert_eq!(config.audio_buffer_chunks().get(), 20);
    }

    #[kithara::test]
    fn an_absent_key_stays_unset_rather_than_defaulting() {
        let audio =
            AudioConfig::<File<TestPools>, NoResamplerBackend>::for_stream(file_config()).build();
        let config = TrackConfig::for_audio(audio)
            .preload_chunks(std::num::NonZeroUsize::new(8).expect("preload"))
            .maybe_audio_buffer_chunks(None)
            .build();
        assert_eq!(
            config.preload_chunks(),
            std::num::NonZeroUsize::new(8).expect("preload")
        );
        assert_eq!(
            config.audio_buffer_chunks(),
            crate::consts::CAPACITY,
            "an unnamed value leaves the owner's capacity unchanged"
        );
    }
}
