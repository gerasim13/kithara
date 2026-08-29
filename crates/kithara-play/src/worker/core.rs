use std::{
    fmt,
    sync::atomic::{AtomicU64, Ordering},
};

use kithara_audio::{Audio, AudioSource, PreparedAudio, ResamplerBackend};
use kithara_bufpool::{BytePool, SamplePool};
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

struct WorkerOwner {
    byte_pool: BytePool,
    sample_pool: SamplePool,
    scheduler: PlaybackScheduler,
}

/// Explicit owner of the shared playback scheduler.
///
/// Clones share one OS thread and one scheduler loop. Dropping a Player only
/// releases that clone; the final owner shuts down the worker.
#[derive(Clone)]
pub struct PlayWorker(Arc<WorkerOwner>);

impl PlayWorker {
    /// Construct the sole playback-worker implementation.
    #[must_use]
    pub fn new(config: PlayWorkerConfig) -> Self {
        let cancel = CancelScope::new(config.cancel).token();
        let id = WORKER_ID.fetch_add(1, Ordering::Relaxed);
        let scheduler =
            PlaybackScheduler::start(format!("kithara-play-worker-{id}"), cancel, config.capacity);
        Self(Arc::new(WorkerOwner {
            byte_pool: config.byte_pool,
            sample_pool: config.sample_pool,
            scheduler,
        }))
    }

    /// Prepare and register a stream-backed audio reader on this worker.
    ///
    /// # Errors
    ///
    /// Returns decode/setup errors or a typed worker registration failure.
    pub async fn open<T, B, C>(&self, config: C) -> DecodeResult<RegisteredAudio<Stream<T>>>
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
        let prepared = Audio::<Stream<T>>::prepare(
            audio,
            Arc::new(wake),
            self.byte_pool().clone(),
            self.sample_pool().clone(),
        )
        .await?;
        let prepared = prepared.map(|audio, source| {
            let spec = audio.spec();
            let warp = Warp::new(audio, &warp);
            let drain = EffectDrain::new(effects.len(), self.byte_pool());
            let source = WarpSource::new(
                source,
                warp.renderer(spec, self.sample_pool().clone()),
                effects,
                drain,
                spec,
            );
            (warp, source)
        });
        self.register(prepared, engine_load)
    }

    fn register<S, P>(
        &self,
        prepared: PreparedAudio<Warp<Audio<S>>, P>,
        engine_load: Option<Arc<EngineLoad>>,
    ) -> DecodeResult<RegisteredAudio<S>>
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

    pub(super) fn unregister(&self, task_id: TaskId) {
        self.0.scheduler.unregister(task_id);
    }

    delegate::delegate! {
        to self.0 {
            /// Shared byte pool used by every registered Player/resource.
            #[field(&byte_pool)]
            #[must_use]
            pub fn byte_pool(&self) -> &BytePool;
            /// Shared sample pool used by every registered Player/resource.
            #[field(&sample_pool)]
            #[must_use]
            pub fn sample_pool(&self) -> &SamplePool;
        }
    }
}

impl fmt::Debug for PlayWorker {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PlayWorker")
            .field("byte_pool", self.byte_pool())
            .field("sample_pool", self.sample_pool())
            .finish_non_exhaustive()
    }
}
