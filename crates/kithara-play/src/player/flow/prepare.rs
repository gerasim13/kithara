use std::num::NonZeroU32;

use kithara_audio::AudioDecoderConfig;
use kithara_platform::sync::Arc;

#[cfg(test)]
use super::super::core::PlayerImpl;
use super::super::core::PlayerRuntime;
use crate::resource::ResourceConfig;

struct ConfigPrep<'a> {
    player: &'a PlayerRuntime,
}

impl ConfigPrep<'_> {
    fn prepare(&self, config: ResourceConfig) -> ResourceConfig {
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
        ResourceConfig {
            bus,
            cancel,
            worker: Some(self.player.core.worker.clone()),
            consumer_wake_mode: self.player.core.engine.consumer_wake_mode(),
            host_sample_rate,
            decoder,
            stretch,
            engine_load: Some(Arc::clone(&self.player.core.engine_load)),
            ..config
        }
    }
}

impl PlayerRuntime {
    /// Apply shared worker, host sample rate, ABR, and bus to a resource
    /// config so the resource integrates with this player's engine.
    ///
    /// Call this before [`Resource::new`](crate::resource::Resource::new) to
    /// ensure the resource shares the player's playback worker and resampler is
    /// pre-initialised with the correct ratio. Callers that want a shared HTTP
    /// pool / tokio runtime must build their own downloader and attach it via
    /// [`ResourceConfig::with_downloader`] before passing the config in.
    #[must_use]
    pub fn prepare_config(&self, config: ResourceConfig) -> ResourceConfig {
        ConfigPrep { player: self }.prepare(config)
    }
}

#[cfg(test)]
mod tests {
    use kithara_assets::AssetStore;
    use kithara_audio::ConsumerWakeMode;
    use kithara_bufpool::{BytePool, SamplePool};
    use kithara_platform::sync::Arc;
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        PlayError, PlayWorker, PlayWorkerConfig,
        player::PlayerConfig,
        session::{Cmd, Reply, SessionDispatcher, testing},
    };

    struct ImmediateSession(Arc<dyn SessionDispatcher>);

    impl SessionDispatcher for ImmediateSession {
        fn consumer_wake_mode(&self) -> ConsumerWakeMode {
            ConsumerWakeMode::ImmediateOffRt
        }

        fn exec(&self, cmd: Cmd) -> Result<Reply, PlayError> {
            self.0.exec(cmd)
        }
    }

    fn resource_config(source: &str) -> ResourceConfig {
        let src = ResourceConfig::parse_src(source).expect("valid test source");
        ResourceConfig::for_src(src)
            .store(AssetStore::builder().build())
            .build()
    }

    fn worker() -> PlayWorker {
        PlayWorker::new(
            PlayWorkerConfig::for_pools(BytePool::default(), SamplePool::default()).build(),
        )
    }

    #[kithara::test]
    fn prepare_config_propagates_session_consumer_wake_mode_to_audio() {
        let session: Arc<dyn SessionDispatcher> =
            Arc::new(ImmediateSession(testing::test_session()));
        let player = PlayerImpl::new(
            PlayerConfig::builder()
                .worker(worker())
                .session(session)
                .build(),
        );

        let prepared = player.prepare_config(resource_config("https://example.com/song.mp3"));
        assert_eq!(
            prepared.consumer_wake_mode,
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
}
