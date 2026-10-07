use std::num::{NonZeroU32, NonZeroUsize};

use kithara_audio::{AudioDecoderConfig, DecoderResamplerSettings, ResamplerOptions};
use kithara_bufpool::HasPool;
use kithara_decode::GaplessMode;
use kithara_events::EventBus;
use kithara_platform::{CancelToken, sync::Arc};
use kithara_warp::WarpConfig;

use crate::{EngineLoad, PlayError, PlayWorker, resource::ResourceConfig, session::SessionOutputView};

/// What every track a deck loads opens with: the worker it renders on, the
/// session output it plays into, and the deck's own playback policy.
///
/// The deck holds one and prepares each track's config with it just before
/// the track loads, so the track reads the session's output as it stands then.
#[derive_where::derive_where(Clone)]
pub struct ResourcePrep<S> {
    pub worker: PlayWorker<S>,
    pub output: SessionOutputView,
    pub bus: EventBus,
    pub cancel: Option<CancelToken>,
    /// The renderer every track starts from; a track starts it at its own
    /// settings.
    pub warp: WarpConfig,
    pub response_budget_frames: Option<NonZeroUsize>,
    pub gapless_mode: GaplessMode,
    pub block_on_underrun: bool,
    pub engine_load: Arc<EngineLoad>,
}

impl<S> ResourcePrep<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    /// Prepares `config` to play on the deck's worker into its session.
    /// Before the session measures its output no deadline can be checked, so
    /// buffer depths sized to the render quantum and response budget overwrite
    /// whatever `audio:` configured only once the output shape is known.
    ///
    /// # Errors
    ///
    /// Returns the session output's refusal of the buffer geometry.
    pub fn prepare<B>(&self, config: ResourceConfig<S, B>) -> Result<ResourceConfig<S, B>, PlayError>
    where
        B: Clone + Default,
    {
        let output = self.output.get();
        let bus = config.bus.or_else(|| Some(self.bus.scoped()));
        let cancel = config
            .cancel
            .or_else(|| self.cancel.clone())
            .map(|parent| parent.child());
        let mut audio = config.audio;
        if let (Some(quantum), Some(shape)) =
            (self.warp.render_quantum_frames(), output.stream_shape)
        {
            let (preload, ring) = shape.playback_buffers(quantum, self.response_budget_frames)?;
            audio.preload_chunks = Some(preload);
            audio.audio_buffer_chunks = Some(ring.get());
        }
        let resampler = match config.decoder.resampler().cloned() {
            Some(settings) => Some(settings),
            None => output
                .stream_shape
                .map(|shape| {
                    let chunk_size =
                        usize::try_from(shape.max_block_frames.get()).map_err(|_| {
                            PlayError::Internal("session output block exceeds usize".into())
                        })?;
                    Ok::<_, PlayError>(
                        DecoderResamplerSettings::builder()
                            .backend(B::default())
                            .options(ResamplerOptions::builder().chunk_size(chunk_size).build())
                            .build(),
                    )
                })
                .transpose()?,
        };
        let decoder = AudioDecoderConfig::builder()
            .backend(config.decoder.backend())
            .gapless_mode(self.gapless_mode)
            .maybe_resampler(resampler)
            .build();
        Ok(ResourceConfig {
            bus,
            cancel,
            worker: Some(self.worker.clone()),
            block_on_underrun: self.block_on_underrun,
            audio,
            host_sample_rate: NonZeroU32::new(output.sample_rate.output()),
            consumer_wake_mode: Some(output.consumer_wake_mode),
            decoder,
            warp: self.warp.clone(),
            engine_load: Some(Arc::clone(&self.engine_load)),
            ..config
        })
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;

    use kithara_assets::AssetStore;
    use kithara_audio::ConsumerWakeMode;
    use kithara_render::rt::{BufferGeometryError, StreamShape};
    use kithara_test_utils::kithara;
    use kithara_warp::WarpConfig;

    use super::*;
    use crate::{
        PlayWorkerConfig, PlaybackResamplerBackend, mock,
        resource::ResourceSrc,
        session::SessionError,
        test_pools::{TestPools, pools},
    };

    fn resource_config(source: &str) -> ResourceConfig<TestPools> {
        let pools = pools();
        let src = ResourceSrc::parse(source).expect("valid test source");
        ResourceConfig::for_src(src)
            .store(AssetStore::builder(pools).build())
            .build()
    }

    fn prep(shape: Option<StreamShape>, warp: WarpConfig) -> ResourcePrep<TestPools> {
        ResourcePrep {
            worker: PlayWorker::new(PlayWorkerConfig::builder(pools()).build()),
            output: mock::output(shape),
            bus: EventBus::new(16),
            cancel: None,
            warp,
            response_budget_frames: None,
            gapless_mode: GaplessMode::default(),
            block_on_underrun: false,
            engine_load: Arc::new(EngineLoad::default()),
        }
    }

    fn prep_with_geometry(
        quantum: usize,
        output_buffer: u32,
        response_budget: usize,
    ) -> ResourcePrep<TestPools> {
        let shape = StreamShape::new(
            NonZeroU32::new(output_buffer).expect("fixture output block is non-zero"),
            mock::SAMPLE_RATE,
        );
        let warp = WarpConfig::builder()
            .render_quantum_frames(NonZeroUsize::new(quantum).expect("fixture quantum is non-zero"))
            .build();
        ResourcePrep {
            response_budget_frames: Some(
                NonZeroUsize::new(response_budget).expect("fixture budget is non-zero"),
            ),
            ..prep(Some(shape), warp)
        }
    }

    #[kithara::test]
    fn prepare_config_sizes_default_resampling_work_to_the_output_block() {
        let shape = StreamShape::new(
            NonZeroU32::new(128).expect("test block is non-zero"),
            mock::SAMPLE_RATE,
        );
        let prepared = prep(Some(shape), WarpConfig::builder().build())
            .prepare(resource_config("https://example.com/song.mp3"))
            .expect("test session answers stream-shape queries");

        assert_eq!(
            prepared
                .decoder
                .resampler()
                .expect("known output shape installs decoder resampling settings")
                .options()
                .chunk_size,
            128
        );
    }

    #[kithara::test]
    fn prepare_config_without_a_measured_output_keeps_default_resampling_work() {
        let prepared = prep(None, WarpConfig::builder().build())
            .prepare(resource_config("https://example.com/song.mp3"))
            .expect("resources may be prepared before the session measures its output");

        assert!(prepared.decoder.resampler().is_none());
    }

    #[kithara::test]
    #[case::default(None, None)]
    #[case::explicit(Some(64), Some(64))]
    fn unmeasured_preparation_preserves_audio_settings_and_resolves_deck_quantum(
        #[case] configured: Option<usize>,
        #[case] expected: Option<usize>,
    ) {
        let prep = prep(
            None,
            WarpConfig::builder()
                .maybe_render_quantum_frames(configured.and_then(NonZeroUsize::new))
                .build(),
        );
        let mut config = resource_config("https://example.com/song.mp3");
        config.audio.preload_chunks = NonZeroUsize::new(7);
        config.audio.audio_buffer_chunks = Some(11);
        let prepared = prep.prepare(config).expect("unmeasured preparation");
        assert_eq!(
            prepared.warp.render_quantum_frames().map(NonZeroUsize::get),
            expected
        );
        assert_eq!(
            prepared.audio.preload_chunks.map(NonZeroUsize::get),
            Some(7)
        );
        assert_eq!(prepared.audio.audio_buffer_chunks, Some(11));
        assert!(prepared.decoder.resampler().is_none());
    }

    #[kithara::test]
    fn prepare_config_preserves_explicit_resampling_work() {
        let explicit = DecoderResamplerSettings::builder()
            .backend(PlaybackResamplerBackend::default())
            .options(ResamplerOptions::builder().chunk_size(256).build())
            .build();
        let mut config = resource_config("https://example.com/song.mp3");
        config.decoder = AudioDecoderConfig::builder().resampler(explicit).build();
        let shape = StreamShape::new(
            NonZeroU32::new(128).expect("test block is non-zero"),
            mock::SAMPLE_RATE,
        );

        let prepared = prep(Some(shape), WarpConfig::builder().build())
            .prepare(config)
            .expect("test session answers stream-shape queries");

        assert_eq!(
            prepared
                .decoder
                .resampler()
                .expect("explicit resampling settings remain installed")
                .options()
                .chunk_size,
            256
        );
    }

    #[kithara::test]
    fn prepare_config_carries_the_sessions_rate_and_wake_mode() {
        let prepared = prep(None, WarpConfig::builder().build())
            .prepare(resource_config("https://example.com/song.mp3"))
            .expect("unmeasured preparation");

        assert_eq!(
            prepared.host_sample_rate.map(NonZeroU32::get),
            Some(mock::SAMPLE_RATE.get())
        );
        assert_eq!(
            prepared.consumer_wake_mode,
            Some(ConsumerWakeMode::RealtimeDeferred)
        );
    }

    #[kithara::test]
    #[case::industry_budget(32, 128, 441, 4, 5)]
    #[case::large_continuity_buffer(64, 512, 639, 8, 9)]
    fn prepare_config_derives_playback_buffering(
        #[case] quantum: usize,
        #[case] output_buffer: u32,
        #[case] response_budget: usize,
        #[case] expected_preload: usize,
        #[case] expected_ring: usize,
    ) {
        let prepared = prep_with_geometry(quantum, output_buffer, response_budget)
            .prepare(resource_config("https://example.com/song.mp3"))
            .expect("fixture geometry fits the response budget");

        assert_eq!(
            prepared.audio.preload_chunks.map(NonZeroUsize::get),
            Some(expected_preload)
        );
        assert_eq!(prepared.audio.audio_buffer_chunks, Some(expected_ring));
    }

    #[kithara::test]
    #[case::one_frame_over_budget(64, 128, 254, 255)]
    #[case::large_buffer_over_industry_budget(64, 512, 441, 639)]
    fn prepare_config_rejects_buffering_over_budget(
        #[case] quantum: usize,
        #[case] output_buffer: u32,
        #[case] response_budget: usize,
        #[case] required_frames: usize,
    ) {
        let prep = prep_with_geometry(quantum, output_buffer, response_budget);

        assert!(matches!(
            prep.prepare(resource_config("https://example.com/song.mp3")),
            Err(PlayError::Session(SessionError::BufferGeometry(
                BufferGeometryError::BudgetExceeded {
                    max_block_frames,
                    render_quantum_frames,
                    required_frames: actual_required_frames,
                    budget_frames,
                }
            ))) if max_block_frames == output_buffer
                && render_quantum_frames == quantum
                && actual_required_frames == required_frames
                && budget_frames == response_budget
        ));
    }
}
