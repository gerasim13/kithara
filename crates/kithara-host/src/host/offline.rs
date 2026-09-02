use std::num::{NonZeroU32, NonZeroUsize};

use bon::Builder;
use kithara_bufpool::{HasPool, PoolRegion};
use kithara_output::{
    OfflineRenderError, OfflineRenderReport, OfflineRenderRequest, OfflineRenderer, RenderSink,
};
use kithara_platform::{CancelToken, sync::Arc, time::Duration};
use kithara_play::{GroupState, PlayError, player::PlayerMember};
use kithara_signal::AudioSpec;
use kithara_worker::{DispatcherConfig, TaskConfig, Worker, WorkerConfig};

use super::{Host, SessionConfig};
use crate::session::{
    HostDispatcher, RootView,
    offline::{OfflineSessionClient, OfflineTaskConfig},
};

struct Defaults;

impl Defaults {
    const CHANNELS: u16 = 2;
    const SAMPLE_RATE: NonZeroU32 = match NonZeroU32::new(44_100) {
        Some(value) => value,
        None => unreachable!(),
    };
    const BLOCK_FRAMES: NonZeroU32 = match NonZeroU32::new(512) {
        Some(value) => value,
        None => unreachable!(),
    };
}

fn default_dispatcher_config() -> DispatcherConfig {
    DispatcherConfig::builder()
        .name("kithara-engine-offline")
        .capacity(NonZeroUsize::MIN)
        .build()
}

/// Configuration for one device-free Host session.
#[derive(Builder)]
#[builder(state_mod(vis = "pub"))]
#[non_exhaustive]
pub struct OfflineSessionConfig<S> {
    /// Typed output pool shared with the Host's players.
    #[builder(start_fn)]
    pub(crate) pools: PoolRegion<S>,
    /// Exact offline output rate.
    #[builder(default = Defaults::SAMPLE_RATE)]
    pub(crate) sample_rate: NonZeroU32,
    /// Maximum frames processed by one backend/task quantum.
    #[builder(default = Defaults::BLOCK_FRAMES)]
    pub(crate) max_block_frames: NonZeroU32,
    /// Firewheel smoothing window for graph changes.
    #[builder(default = Defaults::BLOCK_FRAMES)]
    pub(crate) declick_frames: NonZeroU32,
    /// Declared device-equivalent latency used by transport calculations.
    #[builder(default = Duration::ZERO)]
    pub(crate) declared_latency: Duration,
    /// Shared worker configuration for the session scheduler.
    #[builder(default = WorkerConfig::new())]
    pub(crate) worker: WorkerConfig,
    /// Dispatcher budgets for the single offline session task.
    #[builder(default = default_dispatcher_config())]
    pub(crate) dispatcher: DispatcherConfig,
    /// Admission, priority, and cancellation configuration for the session task.
    #[builder(default = TaskConfig::new())]
    pub(crate) task: TaskConfig,
    /// Optional automatic test/probe render cadence.
    #[cfg(any(test, feature = "probe"))]
    pub(crate) pacing: Option<Duration>,
}

impl<S> OfflineSessionConfig<S> {
    /// Typed output pool shared with every player in this session.
    #[must_use]
    pub const fn pools(&self) -> &PoolRegion<S> {
        &self.pools
    }

    /// Exact offline output rate.
    #[must_use]
    pub const fn sample_rate(&self) -> NonZeroU32 {
        self.sample_rate
    }

    /// Maximum frames processed by one backend/task quantum.
    #[must_use]
    pub const fn max_block_frames(&self) -> NonZeroU32 {
        self.max_block_frames
    }

    /// Firewheel smoothing window for graph changes.
    #[must_use]
    pub const fn declick_frames(&self) -> NonZeroU32 {
        self.declick_frames
    }

    /// Declared device-equivalent latency used by transport calculations.
    #[must_use]
    pub const fn declared_latency(&self) -> Duration {
        self.declared_latency
    }

    /// Optional automatic test/probe render cadence.
    #[cfg(any(test, feature = "probe"))]
    #[must_use]
    pub const fn pacing(&self) -> Option<Duration> {
        self.pacing
    }
}

impl<S> SessionConfig<S> {
    /// Configure a device-free offline session.
    #[must_use]
    pub fn offline(config: OfflineSessionConfig<S>) -> Self {
        Self::Offline(Box::new(config))
    }
}

pub(super) struct OfflineRuntime<S> {
    client: Arc<OfflineSessionClient<S>>,
    max_block_frames: NonZeroU32,
    spec: AudioSpec,
    _worker: Worker,
    _dispatcher: kithara_worker::Dispatcher,
    _task: kithara_worker::TaskHandle,
}

type StartedOfflineRuntime<S> = (Arc<dyn HostDispatcher<S>>, OfflineRuntime<S>);

impl<S> OfflineRuntime<S>
where
    S: HasPool<f32> + Send + Sync + 'static,
{
    pub(super) fn new(
        config: OfflineSessionConfig<S>,
        root: GroupState<PlayerMember>,
        root_view: RootView,
    ) -> Result<StartedOfflineRuntime<S>, PlayError> {
        let sample_rate = config.sample_rate;
        let max_block_frames = config.max_block_frames;
        let spec = AudioSpec::new(Defaults::CHANNELS, sample_rate);
        let worker = Worker::new(config.worker);
        let dispatcher = worker.dispatcher(config.dispatcher);
        let (client, task) = crate::session::offline::spawn(
            &dispatcher,
            config.task,
            root,
            root_view,
            OfflineTaskConfig {
                pools: config.pools,
                sample_rate,
                max_block_frames,
                declick_frames: config.declick_frames,
                declared_latency: config.declared_latency,
                #[cfg(any(test, feature = "probe"))]
                pacing: config.pacing,
            },
        )?;
        let host_dispatcher: Arc<dyn HostDispatcher<S>> = client.clone();
        Ok((
            host_dispatcher,
            Self {
                client,
                max_block_frames,
                spec,
                _worker: worker,
                _dispatcher: dispatcher,
                _task: task,
            },
        ))
    }

    fn position(&self) -> Result<u64, OfflineRenderError> {
        self.client.position().map_err(OfflineRenderError::backend)
    }

    fn render_at(
        &self,
        position: u64,
        frames: u32,
    ) -> Result<kithara_bufpool::SampleBuffer, OfflineRenderError> {
        self.client
            .render(position, frames)
            .map_err(|error| match error {
                crate::session::offline::OfflineSessionError::CursorChanged { actual, .. } => {
                    OfflineRenderError::RangeUnavailable {
                        requested: position,
                        current: actual,
                    }
                }
                error => OfflineRenderError::backend(error),
            })
    }

    fn render(
        &mut self,
        request: &OfflineRenderRequest,
        cancel: &CancelToken,
        sink: &mut dyn RenderSink,
    ) -> Result<OfflineRenderReport, OfflineRenderError> {
        let requested_frames = request.frame_count()?;
        if request.spec() != self.spec {
            return Err(OfflineRenderError::SpecMismatch {
                expected: self.spec,
                actual: request.spec(),
            });
        }
        let mut position = self.position()?;
        if request.frames().start < position {
            return Err(OfflineRenderError::RangeUnavailable {
                requested: request.frames().start,
                current: position,
            });
        }

        while position < request.frames().start {
            if cancel.is_cancelled() {
                return Err(OfflineRenderError::Cancelled { rendered_frames: 0 });
            }
            let remaining = request.frames().start - position;
            let frames = remaining.min(u64::from(self.max_block_frames.get()));
            let frames = u32::try_from(frames).map_err(OfflineRenderError::backend)?;
            let _ = self.render_at(position, frames)?;
            position = position
                .checked_add(u64::from(frames))
                .ok_or_else(|| OfflineRenderError::backend(TimelineOverflow))?;
        }

        let mut rendered_frames = 0;
        while position < request.frames().end {
            if cancel.is_cancelled() {
                return Err(OfflineRenderError::Cancelled { rendered_frames });
            }
            let remaining = request.frames().end - position;
            let frames = remaining.min(u64::from(self.max_block_frames.get()));
            let frames = u32::try_from(frames).map_err(OfflineRenderError::backend)?;
            let block = self.render_at(position, frames)?;
            if cancel.is_cancelled() {
                return Err(OfflineRenderError::Cancelled { rendered_frames });
            }
            sink.write(&block)
                .map_err(|error| OfflineRenderError::sink(rendered_frames, error))?;
            position = position
                .checked_add(u64::from(frames))
                .ok_or_else(|| OfflineRenderError::backend(TimelineOverflow))?;
            rendered_frames = rendered_frames
                .checked_add(u64::from(frames))
                .ok_or_else(|| OfflineRenderError::backend(TimelineOverflow))?;
        }
        if cancel.is_cancelled() {
            return Err(OfflineRenderError::Cancelled { rendered_frames });
        }
        debug_assert_eq!(rendered_frames, requested_frames);
        Ok(OfflineRenderReport::new(rendered_frames))
    }
}

impl<S> OfflineRenderer for Host<S>
where
    S: HasPool<f32> + Send + Sync + 'static,
{
    fn render(
        &mut self,
        request: &OfflineRenderRequest,
        cancel: &CancelToken,
        sink: &mut dyn RenderSink,
    ) -> Result<OfflineRenderReport, OfflineRenderError> {
        self.session
            .offline_runtime_mut()
            .ok_or(OfflineRenderError::SessionModeUnavailable)?
            .render(request, cancel, sink)
    }
}

#[derive(Debug, thiserror::Error)]
#[error("offline Host timeline overflow")]
struct TimelineOverflow;

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use kithara_bufpool::testing::TestPools;
    use kithara_output::{OfflineRenderRequest, OfflineRenderer, RenderSinkError};
    use kithara_platform::CancelScope;
    use kithara_test_utils::kithara;

    use super::*;
    use crate::HostConfig;

    struct Discard;

    impl RenderSink for Discard {
        fn write(&mut self, _samples: &[f32]) -> Result<(), RenderSinkError> {
            Ok(())
        }
    }

    #[kithara::test(native, flash(false))]
    fn realtime_host_rejects_offline_rendering() {
        let mut host =
            Host::<TestPools>::new(HostConfig::builder().build()).expect("fixture realtime Host");
        let request = OfflineRenderRequest::builder()
            .spec(AudioSpec::new(Defaults::CHANNELS, Defaults::SAMPLE_RATE))
            .frames(0..1)
            .build();
        let cancel = CancelScope::new(None);

        assert!(matches!(
            host.render(&request, &cancel.token(), &mut Discard),
            Err(OfflineRenderError::SessionModeUnavailable)
        ));
    }
}
