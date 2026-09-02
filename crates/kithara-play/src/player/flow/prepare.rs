use std::num::NonZeroU32;

use kithara_audio::AudioDecoderConfig;
use kithara_bufpool::HasPool;
use kithara_platform::sync::Arc;

#[cfg(test)]
use super::super::core::PlayerImpl;
use super::super::core::PlayerRuntime;
use crate::resource::ResourceConfig;

struct ConfigPrep<'a, S> {
    player: &'a PlayerRuntime<S>,
}

impl<S> ConfigPrep<'_, S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    fn prepare<B>(&self, config: ResourceConfig<S, B>) -> ResourceConfig<S, B>
    where
        B: Clone + Default,
    {
        let bus = config
            .bus
            .or_else(|| Some(self.player.core.engine.bus().scoped()));
        let cancel = config
            .cancel
            .or_else(|| self.player.core.engine.cancel_token())
            .map(|parent| parent.child());
        let stretch = Arc::clone(&self.player.core.timestretch);
        let host_sample_rate = NonZeroU32::new(self.player.core.engine.master_sample_rate())
            .or_else(|| NonZeroU32::new(self.player.core.engine.configured_sample_rate()));
        let decoder = AudioDecoderConfig::builder()
            .backend(config.decoder.backend())
            .gapless_mode(self.player.core.gapless_mode)
            .maybe_resampler(config.decoder.resampler().cloned())
            .build();
        let mut settings = config.settings.clone();
        settings.audio.consumer_wake_mode = self.player.core.engine.consumer_wake_mode();
        settings.audio.block_on_underrun = self.player.core.block_on_underrun;
        settings.audio.host_sample_rate = host_sample_rate;
        ResourceConfig {
            bus,
            cancel,
            worker: Some(self.player.core.worker.clone()),
            settings,
            decoder,
            stretch,
            engine_load: Some(Arc::clone(&self.player.core.engine_load)),
            ..config
        }
    }
}

impl<S> PlayerRuntime<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    /// Apply shared worker, host sample rate, ABR, and bus to a resource
    /// config so the resource integrates with this player's engine.
    ///
    /// Call this before [`Resource::new`](crate::resource::Resource::new) to
    /// ensure the resource shares the player's playback worker and resampler is
    /// pre-initialised with the correct ratio. Callers that want a shared HTTP
    /// pool / tokio runtime must build their own downloader and attach it via
    /// [`ResourceConfig::with_downloader`] before passing the config in.
    #[must_use]
    pub fn prepare_config<B>(&self, config: ResourceConfig<S, B>) -> ResourceConfig<S, B>
    where
        B: Clone + Default,
    {
        ConfigPrep { player: self }.prepare(config)
    }
}

#[cfg(test)]
mod tests {
    use kithara_assets::AssetStore;
    use kithara_audio::{AudioSettings, ConsumerWakeMode};
    use kithara_platform::sync::Arc;
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        PlayError, PlayWorker, PlayWorkerConfig,
        player::PlayerConfig,
        resource::{ResourceSettings, ResourceSrc},
        session::{Cmd, Reply, SessionDispatcher, testing},
        test_pools::{TestPools, pools},
    };

    struct ImmediateSession(Arc<dyn SessionDispatcher<TestPools>>);

    impl SessionDispatcher<TestPools> for ImmediateSession {
        fn exec(&self, cmd: Cmd<TestPools>) -> Result<Reply, PlayError> {
            self.0.exec(cmd)
        }

        fn consumer_wake_mode(&self) -> ConsumerWakeMode {
            ConsumerWakeMode::ImmediateOffRt
        }
    }

    fn resource_config(source: &str) -> ResourceConfig<TestPools> {
        let pools = pools();
        let src = ResourceSrc::parse(source).expect("valid test source");
        ResourceConfig::for_src(src)
            .store(AssetStore::builder(pools).build())
            .build()
    }

    fn worker() -> PlayWorker<TestPools> {
        PlayWorker::new(PlayWorkerConfig::builder(pools()).build())
    }

    #[kithara::test]
    fn prepare_config_propagates_session_consumer_wake_mode_to_audio() {
        let session: Arc<dyn SessionDispatcher<TestPools>> =
            Arc::new(ImmediateSession(testing::test_session()));
        let player = PlayerImpl::new(
            PlayerConfig::builder()
                .worker(worker())
                .session(session)
                .build(),
        );

        let prepared = player.prepare_config(resource_config("https://example.com/song.mp3"));
        assert_eq!(
            prepared.settings.audio.consumer_wake_mode,
            ConsumerWakeMode::ImmediateOffRt
        );
        let audio = prepared.build_file_config(player.worker(), None);
        assert_eq!(audio.consumer_wake_mode(), ConsumerWakeMode::ImmediateOffRt);

        let prepared = player.prepare_config(resource_config("https://example.com/live.m3u8"));
        let audio = prepared
            .build_hls_config(player.worker(), None)
            .expect("valid HLS config");
        assert_eq!(audio.consumer_wake_mode(), ConsumerWakeMode::ImmediateOffRt);
    }

    #[kithara::test]
    fn prepare_config_overwrites_a_builder_declared_wake_mode() {
        let player = PlayerImpl::new(
            PlayerConfig::builder()
                .worker(worker())
                .session(testing::test_session())
                .build(),
        );

        let src = ResourceSrc::parse("https://example.com/song.mp3").expect("valid test source");
        let config = ResourceConfig::<TestPools>::for_src(src)
            .store(AssetStore::builder(pools()).build())
            .settings(
                ResourceSettings::builder()
                    .audio(
                        AudioSettings::builder()
                            .consumer_wake_mode(ConsumerWakeMode::ImmediateOffRt)
                            .build(),
                    )
                    .build(),
            )
            .build();

        let prepared = player.prepare_config(config);
        assert_eq!(
            prepared.settings.audio.consumer_wake_mode,
            ConsumerWakeMode::RealtimeDeferred,
            "a player-managed resource cannot smuggle an off-RT capability past the session policy"
        );
    }
}
