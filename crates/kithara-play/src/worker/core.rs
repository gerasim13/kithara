use std::{
    fmt,
    sync::atomic::{AtomicU64, Ordering},
};

use kithara_audio::{Audio, AudioConfig, PcmScheduler, PcmTaskId, PreparedAudio, ResamplerBackend};
use kithara_bufpool::{BytePool, PcmPool};
use kithara_decode::{DecodeError, DecodeResult};
use kithara_events::EventBus;
use kithara_platform::{CancelScope, sync::Arc};
use kithara_stream::{Stream, StreamType};

use super::{PlayWorkerConfig, RegisteredAudio, TrackLease};

static WORKER_ID: AtomicU64 = AtomicU64::new(1);

struct WorkerOwner {
    byte_pool: BytePool,
    pcm_pool: PcmPool,
    scheduler: PcmScheduler,
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
            PcmScheduler::start(format!("kithara-play-worker-{id}"), cancel, config.capacity);
        Self(Arc::new(WorkerOwner {
            byte_pool: config.byte_pool,
            pcm_pool: config.pcm_pool,
            scheduler,
        }))
    }

    /// Prepare and register a stream-backed audio reader on this worker.
    ///
    /// # Errors
    ///
    /// Returns decode/setup errors or a typed worker registration failure.
    pub async fn open<T, B>(
        &self,
        config: AudioConfig<T, B>,
    ) -> DecodeResult<RegisteredAudio<Stream<T>>>
    where
        T: StreamType<Events = EventBus>,
        B: Default + ResamplerBackend,
    {
        let wake = self.0.scheduler.wake_handle();
        let prepared = Audio::<Stream<T>>::prepare(
            config,
            wake,
            self.byte_pool().clone(),
            self.pcm_pool().clone(),
        )
        .await?;
        self.register(prepared)
    }

    fn register<S>(&self, prepared: PreparedAudio<S>) -> DecodeResult<RegisteredAudio<S>> {
        let (audio, task) = prepared.into();
        let task_id = self
            .0
            .scheduler
            .register(task)
            .map_err(|error| DecodeError::pcm_stream("play worker registration", error))?;
        Ok(RegisteredAudio::new(
            audio,
            TrackLease::new(self.clone(), task_id),
        ))
    }

    pub(super) fn unregister(&self, task_id: PcmTaskId) {
        self.0.scheduler.unregister(task_id);
    }

    delegate::delegate! {
        to self.0 {
            /// Shared byte pool used by every registered Player/resource.
            #[field(&byte_pool)]
            #[must_use]
            pub fn byte_pool(&self) -> &BytePool;
            /// Shared PCM pool used by every registered Player/resource.
            #[field(&pcm_pool)]
            #[must_use]
            pub fn pcm_pool(&self) -> &PcmPool;
        }
    }
}

impl fmt::Debug for PlayWorker {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PlayWorker")
            .field("byte_pool", self.byte_pool())
            .field("pcm_pool", self.pcm_pool())
            .finish_non_exhaustive()
    }
}
