use std::num::NonZeroU32;

use kithara_bufpool::PcmPool;
use kithara_decode::{PcmChunk, PcmSpec};
use kithara_test_utils::kithara;

use super::create_effects;
use crate::{effects::timestretch::StretchControls, traits::AudioEffect};

struct PassthroughEffect;

impl AudioEffect for PassthroughEffect {
    fn flush(&mut self) -> Option<PcmChunk> {
        None
    }
    fn process(&mut self, chunk: PcmChunk) -> Option<PcmChunk> {
        Some(chunk)
    }
    fn reset(&mut self) {}
}

fn spec() -> PcmSpec {
    PcmSpec::new(2, NonZeroU32::new(44100).expect("test rate"))
}

fn pool() -> PcmPool {
    PcmPool::default()
}

#[kithara::test]
fn create_effects_includes_custom_effects() {
    let pool = pool();
    let effects = create_effects(spec(), None, &pool, vec![Box::new(PassthroughEffect)]);
    assert_eq!(effects.len(), 1);
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use kithara_assets::{AssetStore, StorageBackend};
    use kithara_file::{FileConfig, FileSrc};
    use kithara_resampler::NoResamplerBackend;
    use unimock::Unimock;

    use super::*;
    use crate::pipeline::config::{AudioConfig, ConsumerWakeMode};

    fn file_config() -> FileConfig {
        FileConfig::for_src(FileSrc::Local(
            std::env::temp_dir().join("kithara-audio-config.wav"),
        ))
        .store(
            AssetStore::builder()
                .backend(StorageBackend::Memory)
                .build(),
        )
        .build()
    }

    #[kithara::test]
    fn audio_config_with_effect_adds_to_chain() {
        let effects: Vec<Box<dyn AudioEffect>> =
            vec![Box::new(PassthroughEffect), Box::new(PassthroughEffect)];
        let config =
            AudioConfig::<kithara_file::File, NoResamplerBackend>::for_stream(file_config())
                .effects(effects)
                .build();
        assert_eq!(config.effects().len(), 2);
    }

    #[kithara::test]
    fn audio_config_defaults_to_realtime_deferred_consumer_wakes() {
        let config =
            AudioConfig::<kithara_file::File, NoResamplerBackend>::for_stream(file_config())
                .build();

        assert_eq!(
            config.consumer_wake_mode(),
            ConsumerWakeMode::RealtimeDeferred
        );
    }

    #[kithara::test]
    fn audio_config_observer_is_optional_and_configurable() {
        let default =
            AudioConfig::<kithara_file::File, NoResamplerBackend>::for_stream(file_config())
                .build();
        let config =
            AudioConfig::<kithara_file::File, NoResamplerBackend>::for_stream(file_config())
                .observer(Box::new(Unimock::new(())))
                .build();

        assert!(default.observer.is_none());
        assert!(config.observer.is_some());
    }
}

#[cfg(target_arch = "wasm32")]
mod no_stretch {
    use super::*;

    /// Without a compiled-in stretch backend, `stretch` does not add a speed
    /// slot: playback remains at unity.
    #[kithara::test]
    fn create_effects_stretch_without_backends_keeps_chain_empty() {
        let controls = StretchControls::new(1.5);
        let pool = pool();
        let effects = create_effects(spec(), Some(&controls), &pool, Vec::new());
        assert!(effects.is_empty());
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod stretch {
    use kithara_decode::PcmMeta;
    use kithara_stretch::StretchKind;

    use super::*;

    #[kithara::test]
    fn create_effects_tempo_mode_prepends_stretch_slot() {
        let controls = StretchControls::new(1.0);
        let pool = pool();
        let effects = create_effects(
            spec(),
            Some(&controls),
            &pool,
            vec![Box::new(PassthroughEffect)],
        );
        assert_eq!(effects.len(), 2);
    }

    /// Key-lock off in tempo mode is still handled by the stretch slot.
    #[kithara::test]
    #[cfg_attr(
        feature = "stretch-signalsmith",
        case::signalsmith(StretchKind::Signalsmith)
    )]
    #[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
    fn create_effects_tempo_vinyl_uses_stretch_slot(#[case] backend: StretchKind) {
        let controls = StretchControls::new(1.5);
        controls.set_keylock(false);
        controls.set_backend(backend);
        let pool = pool();
        let mut effects = create_effects(spec(), Some(&controls), &pool, Vec::new());
        // Drive one chunk through the stretch slot (index 0).
        let frames = 1024usize;
        let samples = vec![0.0_f32; frames * 2];
        let meta = PcmMeta {
            spec: spec(),
            frames: u32::try_from(frames).unwrap(),
            ..Default::default()
        };
        let chunk = PcmChunk::new(meta, PcmPool::default().attach(samples));
        let out = effects[0]
            .process(chunk)
            .expect("vinyl stretch emits a chunk");
        assert_eq!(out.spec(), spec());
        assert!(!out.samples.is_empty());
    }
}
