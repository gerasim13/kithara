use std::{
    fmt,
    sync::atomic::{AtomicU64, Ordering},
};

use kithara_audio::{Audio, AudioSource, PreparedAudio, ResamplerBackend};
use kithara_bufpool::{BytePool, SamplePool};
use kithara_decode::{DecodeError, DecodeResult};
use kithara_events::EventBus;
use kithara_platform::{CancelGroup, CancelToken, sync::Arc};
use kithara_stream::{Stream, StreamType};
use kithara_warp::Warp;
use kithara_worker::{Dispatcher, DispatcherConfig, TaskConfig, Worker, WorkerConfig};

use super::{
    DecoderNode, EngineLoad, PlayWorkerConfig, RegisteredAudio, TrackConfig, TrackLease,
    WarpSource,
    scheduler::{PlaybackObserver, ServiceClass, Wake},
};
use crate::effects::EffectDrain;

static WORKER_ID: AtomicU64 = AtomicU64::new(1);

struct WorkerOwner {
    byte_pool: BytePool,
    dispatcher: Dispatcher,
    sample_pool: SamplePool,
    base: Worker,
}

/// Explicit owner of the playback dispatcher.
///
/// Clones share one OS thread and one scheduler loop. Dropping a Player only
/// releases that clone; the final owner shuts down its dispatcher and releases
/// its base-worker clone.
#[derive(Clone)]
pub struct PlayWorker(Arc<WorkerOwner>);

impl PlayWorker {
    /// Construct the sole playback-worker implementation.
    #[must_use]
    pub fn new(config: PlayWorkerConfig) -> Self {
        let PlayWorkerConfig {
            byte_pool,
            cancel,
            capacity,
            fairness_yield_interval,
            idle_timeout,
            sample_pool,
            slow_tick_threshold,
            task_burst,
            wait_timeout,
            worker,
        } = config;
        let (base, dispatcher_cancel) = if let Some(worker) = worker {
            (worker, cancel.map(CancelGroup::from))
        } else {
            let worker_config = cancel.map_or_else(WorkerConfig::new, |cancel| {
                WorkerConfig::new().with_cancel(cancel)
            });
            (Worker::new(worker_config), None)
        };
        let id = WORKER_ID.fetch_add(1, Ordering::Relaxed);
        let mut dispatcher_config = DispatcherConfig::new(format!("kithara-play-worker-{id}"))
            .with_capacity(capacity)
            .with_fairness_yield_interval(fairness_yield_interval)
            .with_idle_timeout(idle_timeout)
            .with_observer(PlaybackObserver::default())
            .with_slow_tick_threshold(slow_tick_threshold)
            .with_task_burst(task_burst)
            .with_wait_timeout(wait_timeout);
        if let Some(cancel) = dispatcher_cancel {
            dispatcher_config = dispatcher_config.with_cancel(cancel);
        }
        let dispatcher = base.dispatcher(dispatcher_config);
        Self(Arc::new(WorkerOwner {
            byte_pool,
            dispatcher,
            sample_pool,
            base,
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
        let task_cancel = audio.cancel().cloned();
        let wake = Wake::new(self.0.dispatcher.wake_handle());
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
        self.register(prepared, engine_load, task_cancel)
    }

    fn register<S, P>(
        &self,
        prepared: PreparedAudio<Warp<Audio<S>>, P>,
        engine_load: Option<Arc<EngineLoad>>,
        cancel: Option<CancelToken>,
    ) -> DecodeResult<RegisteredAudio<S>>
    where
        P: AudioSource<Chunk = kithara_signal::AudioChunk>,
    {
        let (audio, lane) = prepared.into();
        let mut task_config = TaskConfig::new().with_priority(ServiceClass::default().into());
        if let Some(cancel) = cancel {
            task_config = task_config.with_cancel(CancelGroup::from(cancel));
        }
        let task = self
            .0
            .dispatcher
            .register(task_config, |_| DecoderNode::new(lane, engine_load))
            .map_err(|error| DecodeError::audio_stream("play worker registration", error))?;
        Ok(RegisteredAudio::new(
            audio,
            TrackLease::new(self.clone(), task),
        ))
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
            .field("base_cancelled", &self.0.base.is_cancelled())
            .field("byte_pool", self.byte_pool())
            .field("sample_pool", self.sample_pool())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use kithara_bufpool::Region;
    use kithara_platform::CancelScope;
    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test]
    fn shared_base_outlives_play_dispatcher_and_play_cancel_stays_local() {
        let base = Worker::new(WorkerConfig::new());
        let cancel = CancelScope::new(None);
        let region = Region::default();
        let play = PlayWorker::new(
            PlayWorkerConfig::for_pools(region.byte_pool(), region.sample_pool())
                .worker(base.clone())
                .cancel(cancel.token())
                .build(),
        );

        cancel.cancel();

        assert!(play.0.dispatcher.is_cancelled());
        assert!(!base.is_cancelled());
        drop(play);
        assert!(!base.is_cancelled());
    }
}
