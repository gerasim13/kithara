use std::{
    fmt,
    sync::atomic::{AtomicU64, Ordering},
};

use kithara_audio::{Audio, ResamplerBackend};
use kithara_bufpool::{HasPool, PoolRegion};
use kithara_command::{ChannelConfig, channel};
use kithara_decode::{DecodeError, DecodeResult};
use kithara_effects::EffectDrain;
use kithara_events::EventBus;
use kithara_platform::{CancelGroup, CancelToken, sync::Arc};
use kithara_render::{LaneProtocol, ServiceClass, WarpSource};
use kithara_stream::{Stream, StreamType};
use kithara_warp::Warp;
use kithara_worker::{
    Dispatcher, DispatcherConfig, PendingTask, TaskConfig, TaskError, Worker, WorkerConfig,
};

use super::{
    DecoderNode, PlayWorkerConfig, RegisteredAudio, TrackConfig, TrackLease,
    scheduler::{PlaybackObserver, Wake},
};

static WORKER_ID: AtomicU64 = AtomicU64::new(1);

/// Why a worker refused to load a track.
#[derive(Debug, thiserror::Error)]
pub enum LoadRefusal {
    /// Every worker slot is held; the track's source was not opened.
    #[error("play worker holds its capacity of {capacity} tracks")]
    Capacity { capacity: usize },
    /// The track did not open: its source or decoder failed, the load was
    /// cancelled, or the worker stopped.
    #[error(transparent)]
    Open(#[from] DecodeError),
}

impl From<LoadRefusal> for DecodeError {
    fn from(refusal: LoadRefusal) -> Self {
        match refusal {
            LoadRefusal::Open(error) => error,
            capacity @ LoadRefusal::Capacity { .. } => {
                Self::audio_stream("play worker load", capacity)
            }
        }
    }
}

struct WorkerOwner<S> {
    /// Sizes of the channel each registered track's render lane gets.
    lane: ChannelConfig,
    dispatcher: Dispatcher,
    pools: PoolRegion<S>,
    base: Worker,
}

/// Explicit owner of the playback dispatcher.
///
/// Clones share one OS thread and one scheduler loop. Dropping a Player only
/// releases that clone; the final owner shuts down its dispatcher and releases
/// its base-worker clone.
#[derive_where::derive_where(Clone)]
pub struct PlayWorker<S>(Arc<WorkerOwner<S>>);

impl<S> PlayWorker<S> {
    /// Construct the sole playback-worker implementation.
    #[must_use]
    pub fn new(config: PlayWorkerConfig<S>) -> Self {
        let PlayWorkerConfig {
            backpressure_poll_interval,
            cancel,
            capacity,
            fairness_yield_interval,
            idle_timeout,
            lane_capacity,
            pools,
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
        let dispatcher_config = DispatcherConfig::builder()
            .name(format!("kithara-play-worker-{id}"))
            .backpressure_poll_interval(backpressure_poll_interval)
            .capacity(capacity)
            .fairness_yield_interval(fairness_yield_interval)
            .idle_timeout(idle_timeout)
            .observer(PlaybackObserver::default())
            .slow_tick_threshold(slow_tick_threshold)
            .task_burst(task_burst)
            .wait_timeout(wait_timeout)
            .maybe_cancel(dispatcher_cancel)
            .build();
        let dispatcher = base.dispatcher(dispatcher_config);
        Self(Arc::new(WorkerOwner {
            lane: ChannelConfig::builder().capacity(lane_capacity).build(),
            dispatcher,
            pools,
            base,
        }))
    }

    /// Shared typed pool facade used by every registered Player/resource.
    #[must_use]
    pub fn pools(&self) -> &PoolRegion<S> {
        &self.0.pools
    }

    pub(crate) fn wake(&self) {
        self.0.dispatcher.wake_handle().wake();
    }
}

impl<S> PlayWorker<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    /// Loads a stream-backed track: holds its worker slot first, then opens
    /// its source once.
    ///
    /// # Errors
    ///
    /// Returns [`LoadRefusal::Capacity`] without opening anything when every
    /// slot is held, or [`LoadRefusal::Open`] when the track does not open.
    pub async fn load<T, B, C>(
        &self,
        config: C,
    ) -> Result<RegisteredAudio<Stream<T>, S>, LoadRefusal>
    where
        T: StreamType<Events = EventBus>,
        B: Default + ResamplerBackend,
        C: Into<TrackConfig<T, B>>,
    {
        let config = config.into();
        let slot = self
            .0
            .dispatcher
            .reserve(Self::task_config(config.audio.cancel().cloned()))
            .map_err(|error| match error {
                TaskError::Capacity { capacity } => LoadRefusal::Capacity { capacity },
                error => LoadRefusal::Open(DecodeError::audio_stream("play worker load", error)),
            })?;
        Ok(self.open_lane(config, slot).await?)
    }

    /// Opens the lane's source and starts it in its held `slot`.
    async fn open_lane<T, B>(
        &self,
        config: TrackConfig<T, B>,
        slot: PendingTask,
    ) -> DecodeResult<RegisteredAudio<Stream<T>, S>>
    where
        T: StreamType<Events = EventBus>,
        B: Default + ResamplerBackend,
    {
        let TrackConfig {
            audio,
            effects,
            engine_load,
            warp,
        } = config;
        let wake = Wake::new(self.0.dispatcher.wake_handle());
        let prepared =
            Audio::<Stream<T>>::prepare(audio, Arc::new(wake), self.pools().clone()).await?;
        let drain = EffectDrain::new(effects.len(), self.pools())?;
        let (lane_sender, inbox) = channel::<LaneProtocol>(self.0.lane);
        let prepared = prepared.map(|audio, source| {
            let spec = audio.spec();
            let warp = Warp::new(audio, &warp);
            let source = WarpSource::new(
                source,
                warp.renderer(spec, self.pools().clone()),
                effects,
                drain,
                spec,
                self.pools().clone(),
                inbox,
            );
            (warp, source)
        });
        let (audio, lane) = prepared.into();
        let task = slot
            .start(|_| DecoderNode::new(lane, engine_load))
            .map_err(|error| DecodeError::audio_stream("play worker start", error))?;
        Ok(RegisteredAudio::new(
            audio,
            TrackLease::new(self.clone(), task),
            lane_sender,
        ))
    }

    fn task_config(cancel: Option<CancelToken>) -> TaskConfig {
        let config = TaskConfig::new().with_priority(ServiceClass::default().into());
        match cancel {
            Some(cancel) => config.with_cancel(CancelGroup::from(cancel)),
            None => config,
        }
    }
}

impl<S> fmt::Debug for PlayWorker<S> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PlayWorker")
            .field("base_cancelled", &self.0.base.is_cancelled())
            .field("pools", self.pools())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use kithara_platform::CancelScope;
    use kithara_test_utils::kithara;

    use super::*;
    use crate::test_pools::pools;

    #[kithara::test]
    fn shared_base_outlives_play_dispatcher_and_play_cancel_stays_local() {
        let base = Worker::new(WorkerConfig::new());
        let cancel = CancelScope::new(None);
        let play = PlayWorker::new(
            PlayWorkerConfig::builder(pools())
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
