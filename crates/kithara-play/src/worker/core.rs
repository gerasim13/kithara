use std::{
    fmt,
    sync::atomic::{AtomicU64, Ordering},
};

use kithara_audio::{Audio, AudioSource, PreparedAudio, ResamplerBackend};
use kithara_bufpool::{HasPool, PoolRegion};
use kithara_decode::{DecodeError, DecodeResult};
use kithara_events::EventBus;
use kithara_platform::{CancelScope, sync::Arc};
use kithara_stream::{Stream, StreamType};
use kithara_warp::Warp;

use super::{
    DecoderNode, EngineLoad, PlayWorkerConfig, RegisteredAudio, TrackConfig, TrackLease,
    WarpSource,
    scheduler::{AtomicServiceClass, PlaybackScheduler, ServiceClass, TaskId},
};
use crate::effects::EffectDrain;

static WORKER_ID: AtomicU64 = AtomicU64::new(1);

struct WorkerOwner<S> {
    pools: PoolRegion<S>,
    scheduler: PlaybackScheduler,
}

/// Explicit owner of the shared playback scheduler.
///
/// Clones share one OS thread and one scheduler loop. Dropping a Player only
/// releases that clone; the final owner shuts down the worker.
pub struct PlayWorker<S>(Arc<WorkerOwner<S>>);

impl<S> PlayWorker<S> {
    /// Construct the sole playback-worker implementation.
    #[must_use]
    pub fn new(config: PlayWorkerConfig<S>) -> Self {
        let cancel = CancelScope::new(config.cancel).token();
        let id = WORKER_ID.fetch_add(1, Ordering::Relaxed);
        let scheduler =
            PlaybackScheduler::start(format!("kithara-play-worker-{id}"), cancel, config.capacity);
        Self(Arc::new(WorkerOwner {
            pools: config.pools,
            scheduler,
        }))
    }

    /// Shared typed pool facade used by every registered Player/resource.
    #[must_use]
    pub fn pools(&self) -> &PoolRegion<S> {
        &self.0.pools
    }

    pub(super) fn unregister(&self, task_id: TaskId) {
        self.0.scheduler.unregister(task_id);
    }
}

impl<S> Clone for PlayWorker<S> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<S> PlayWorker<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    /// Prepare and register a stream-backed audio reader on this worker.
    ///
    /// # Errors
    ///
    /// Returns decode/setup errors or a typed worker registration failure.
    pub async fn open<T, B, C>(&self, config: C) -> DecodeResult<RegisteredAudio<Stream<T>, S>>
    where
        T: StreamType<Events = EventBus>,
        B: Default + ResamplerBackend,
        C: Into<TrackConfig<T, B>>,
    {
        let TrackConfig {
            audio,
            effects,
            engine_load,
            warp,
        } = config.into();
        let wake = self.0.scheduler.wake_handle();
        let prepared =
            Audio::<Stream<T>>::prepare(audio, Arc::new(wake), self.pools().clone()).await?;
        let drain = EffectDrain::new(effects.len(), self.pools())?;
        let prepared = prepared.map(|audio, source| {
            let spec = audio.spec();
            let warp = Warp::new(audio, &warp);
            let source = WarpSource::new(
                source,
                warp.renderer(spec, self.pools().clone()),
                effects,
                drain,
                spec,
            );
            (warp, source)
        });
        self.register(prepared, engine_load)
    }

    fn register<T, P>(
        &self,
        prepared: PreparedAudio<Warp<Audio<T>>, P>,
        engine_load: Option<Arc<EngineLoad>>,
    ) -> DecodeResult<RegisteredAudio<T, S>>
    where
        P: AudioSource<Chunk = kithara_signal::AudioChunk>,
    {
        let (audio, lane) = prepared.into();
        let service_class = Arc::new(AtomicServiceClass::new(ServiceClass::default()));
        let task_id = self
            .0
            .scheduler
            .register(DecoderNode::new(
                lane,
                engine_load,
                Arc::clone(&service_class),
            ))
            .map_err(|error| DecodeError::audio_stream("play worker registration", error))?;
        Ok(RegisteredAudio::new(
            audio,
            TrackLease::new(
                self.clone(),
                task_id,
                service_class,
                self.0.scheduler.wake_handle(),
            ),
        ))
    }
}

impl<S> fmt::Debug for PlayWorker<S> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PlayWorker")
            .field("pools", self.pools())
            .finish_non_exhaustive()
    }
}
