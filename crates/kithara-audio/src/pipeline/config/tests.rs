#[cfg(not(target_arch = "wasm32"))]
mod native {
    use kithara_assets::{AssetStore, StorageBackend};
    use kithara_file::{FileConfig, FileSrc};
    use kithara_resampler::NoResamplerBackend;
    use kithara_test_utils::kithara;
    use unimock::Unimock;

    use crate::{
        pipeline::config::{AudioConfig, ConsumerWakeMode},
        test_pools::{TestPools, pools},
    };

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
    fn audio_config_defaults_to_realtime_deferred_consumer_wakes() {
        let config = AudioConfig::<kithara_file::File<TestPools>, NoResamplerBackend>::for_stream(
            file_config(),
        )
        .build();

        assert_eq!(
            config.consumer_wake_mode(),
            ConsumerWakeMode::RealtimeDeferred
        );
    }

    #[kithara::test]
    fn audio_config_keeps_the_native_ring_and_preload_defaults() {
        let config = AudioConfig::<kithara_file::File<TestPools>, NoResamplerBackend>::for_stream(
            file_config(),
        )
        .build();

        assert_eq!(config.audio_buffer_chunks(), 10);
        assert_eq!(config.preload_chunks().get(), 3);
    }

    #[kithara::test]
    fn audio_config_observer_is_optional_and_configurable() {
        let default = AudioConfig::<kithara_file::File<TestPools>, NoResamplerBackend>::for_stream(
            file_config(),
        )
        .build();
        let config = AudioConfig::<kithara_file::File<TestPools>, NoResamplerBackend>::for_stream(
            file_config(),
        )
        .observer(Box::new(Unimock::new(())))
        .build();

        assert!(default.observer.is_none());
        assert!(config.observer.is_some());
    }
}
