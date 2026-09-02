use std::num::NonZeroU32;

use firewheel::{
    StreamInfo,
    backend::{AudioBackend, BackendProcessInfo},
    node::StreamStatus,
    processor::FirewheelProcessor,
};
use kithara_audio::ConsumerWakeMode;
use kithara_bufpool::{HasPool, PoolRegion, SampleBuffer};
use kithara_platform::{
    sync::{Arc, Mutex, mpsc},
    thread::spawn_named,
    time::{Duration, Instant},
};
use kithara_play::{GroupState, PlayError, player::PlayerMember};
use thiserror::Error;
use tracing::warn;

use super::{
    dispatch::run_host_cmd,
    protocol::{
        Cmd, HostCmd, HostCmdMsg, HostDispatchError, HostDispatcher, HostReply, Reply,
        SessionDispatcher,
    },
    state::{RootView, SessionState, ensure_ctx},
};

const CHANNELS: usize = 2;

#[derive(Clone, Copy)]
struct BackendConfig {
    block_frames: NonZeroU32,
    sample_rate: NonZeroU32,
}

impl Default for BackendConfig {
    fn default() -> Self {
        Self {
            block_frames: NonZeroU32::new(512).unwrap_or(NonZeroU32::MIN),
            sample_rate: NonZeroU32::new(44_100).unwrap_or(NonZeroU32::MIN),
        }
    }
}

struct OfflineBackend {
    frames_rendered: u64,
    processor: Option<FirewheelProcessor<Self>>,
    sample_rate: NonZeroU32,
}

impl OfflineBackend {
    fn render(&mut self, frames: usize, output: &mut [f32]) -> Result<(), OfflineSessionError> {
        let processor = self
            .processor
            .as_mut()
            .ok_or(OfflineSessionError::ProcessorUnavailable)?;
        let rate = u64::from(self.sample_rate.get());
        let whole_seconds = self.frames_rendered / rate;
        let remainder = u32::try_from(self.frames_rendered % rate)
            .map_err(|_| OfflineSessionError::TimelineOverflow)?;
        let process_info = BackendProcessInfo {
            frames,
            num_in_channels: 0,
            num_out_channels: CHANNELS,
            process_timestamp: Instant::now(),
            duration_since_stream_start: Duration::from_secs(whole_seconds)
                + Duration::from_secs_f64(f64::from(remainder) / f64::from(self.sample_rate.get())),
            input_stream_status: StreamStatus::empty(),
            output_stream_status: StreamStatus::empty(),
            dropped_frames: 0,
        };
        processor.process_interleaved(&[], output, process_info);
        self.frames_rendered = self
            .frames_rendered
            .checked_add(u64::try_from(frames).map_err(|_| OfflineSessionError::TimelineOverflow)?)
            .ok_or(OfflineSessionError::TimelineOverflow)?;
        Ok(())
    }
}

impl AudioBackend for OfflineBackend {
    type Config = BackendConfig;
    type Enumerator = ();
    type Instant = Instant;
    type StartStreamError = OfflineSessionError;
    type StreamError = OfflineSessionError;

    fn delay_from_last_process(&self, _process_timestamp: Self::Instant) -> Option<Duration> {
        None
    }

    fn enumerator() -> Self::Enumerator {}

    fn poll_status(&mut self) -> Result<(), Self::StreamError> {
        Ok(())
    }

    fn set_processor(&mut self, processor: FirewheelProcessor<Self>) {
        self.processor = Some(processor);
    }

    fn start_stream(config: Self::Config) -> Result<(Self, StreamInfo), Self::StartStreamError> {
        let stream = StreamInfo {
            sample_rate: config.sample_rate,
            sample_rate_recip: 1.0 / f64::from(config.sample_rate.get()),
            prev_sample_rate: config.sample_rate,
            max_block_frames: config.block_frames,
            num_stream_in_channels: 0,
            num_stream_out_channels: u32::try_from(CHANNELS)
                .map_err(|_| OfflineSessionError::ChannelCountOverflow)?,
            input_to_output_latency_seconds: 0.0,
            declick_frames: config.block_frames,
            output_device_id: "offline".to_owned(),
            input_device_id: None,
        };
        Ok((
            Self {
                frames_rendered: 0,
                processor: None,
                sample_rate: config.sample_rate,
            },
            stream,
        ))
    }
}

enum OfflineMsg<S> {
    Host(HostCmdMsg<S>),
    Render {
        frames: u32,
        reply_tx: mpsc::Sender<Result<SampleBuffer, OfflineSessionError>>,
    },
}

pub(crate) struct OfflineSessionClient<S> {
    cmd_tx: Mutex<mpsc::Sender<OfflineMsg<S>>>,
}

impl<S> OfflineSessionClient<S> {
    fn call(&self, cmd: HostCmd<S>) -> Result<HostReply, HostDispatchError<S>> {
        let (reply_tx, reply_rx) = mpsc::channel();
        let message = OfflineMsg::Host(HostCmdMsg { cmd, reply_tx });
        if let Err(error) = self.cmd_tx.lock().send(message) {
            let OfflineMsg::Host(message) = error.0 else {
                return Err(HostDispatchError::after_send(PlayError::Internal(
                    "offline Host command changed protocol variant before send".into(),
                )));
            };
            return Err(HostDispatchError::before_send(
                PlayError::SessionGone {
                    reason: "offline session stopped accepting commands",
                },
                message.cmd,
            ));
        }
        reply_rx.recv().map_err(|_| {
            HostDispatchError::after_send(PlayError::SessionGone {
                reason: "offline session dropped the reply channel",
            })
        })
    }

    pub(crate) fn render(&self, frames: u32) -> Result<SampleBuffer, OfflineSessionError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.cmd_tx
            .lock()
            .send(OfflineMsg::Render { frames, reply_tx })
            .map_err(|_| OfflineSessionError::SessionGone)?;
        reply_rx
            .recv()
            .map_err(|_| OfflineSessionError::SessionGone)?
    }
}

impl<S: Send + Sync + 'static> SessionDispatcher<S> for OfflineSessionClient<S> {
    fn exec(&self, cmd: Cmd<S>) -> Result<Reply, PlayError> {
        match self.call(HostCmd::Play(cmd)).map_err(PlayError::from)? {
            HostReply::Play(reply) => Ok(reply),
            HostReply::Err(error) => Err(error),
            _ => Err(PlayError::Internal(
                "unexpected offline Host reply for player command".into(),
            )),
        }
    }

    fn consumer_wake_mode(&self) -> ConsumerWakeMode {
        ConsumerWakeMode::RealtimeDeferred
    }
}

impl<S: Send + Sync + 'static> HostDispatcher<S> for OfflineSessionClient<S> {
    fn exec_host(&self, cmd: HostCmd<S>) -> Result<HostReply, HostDispatchError<S>> {
        self.call(cmd)
    }
}

pub(crate) fn spawn<S>(
    root: GroupState<PlayerMember>,
    root_view: RootView,
    sample_rate: NonZeroU32,
    block_frames: NonZeroU32,
    pools: PoolRegion<S>,
) -> Arc<OfflineSessionClient<S>>
where
    S: HasPool<f32> + Send + Sync + 'static,
{
    let (cmd_tx, cmd_rx) = mpsc::channel();
    spawn_named("kithara-engine-offline", move || {
        let start_stream = move |ctx: &mut firewheel::FirewheelCtx<OfflineBackend>, rate: u32| {
            let rate = NonZeroU32::new(rate)
                .ok_or_else(|| "offline sample rate must be non-zero".to_owned())?;
            ctx.start_stream(BackendConfig {
                block_frames,
                sample_rate: rate,
            })
            .map_err(|error| error.to_string())
        };
        let mut state = SessionState::new(root, root_view, sample_rate, start_stream);
        run(&cmd_rx, &mut state, &pools);
    });
    Arc::new(OfflineSessionClient {
        cmd_tx: Mutex::new(cmd_tx),
    })
}

fn run<S>(
    cmd_rx: &mpsc::Receiver<OfflineMsg<S>>,
    state: &mut SessionState<OfflineBackend, S>,
    pools: &PoolRegion<S>,
) where
    S: HasPool<f32> + Send + Sync + 'static,
{
    while let Ok(message) = cmd_rx.recv() {
        match message {
            OfflineMsg::Host(HostCmdMsg { cmd, reply_tx }) => {
                if matches!(cmd, HostCmd::Shutdown) {
                    if reply_tx.send(HostReply::Ok).is_err() {
                        warn!("offline Host shutdown reply receiver dropped");
                    }
                    return;
                }
                let reply = run_host_cmd(state, cmd);
                if reply_tx.send(reply).is_err() {
                    warn!("offline Host command reply receiver dropped");
                }
            }
            OfflineMsg::Render { frames, reply_tx } => {
                let reply = render_block(state, frames, pools);
                if reply_tx.send(reply).is_err() {
                    warn!("offline render reply receiver dropped");
                }
            }
        }
    }
}

fn render_block<S>(
    state: &mut SessionState<OfflineBackend, S>,
    frames: u32,
    pools: &PoolRegion<S>,
) -> Result<SampleBuffer, OfflineSessionError>
where
    S: HasPool<f32> + Send + Sync + 'static,
{
    if state.ctx.is_none() {
        ensure_ctx(state, state.sample_rate_hint)
            .map_err(|error| OfflineSessionError::Graph(error.to_string()))?;
    }
    let total_samples = usize::try_from(frames)
        .map_err(|_| OfflineSessionError::SampleCountOverflow)?
        .checked_mul(CHANNELS)
        .ok_or(OfflineSessionError::SampleCountOverflow)?;
    let mut output = pools
        .get_with_len::<f32>(total_samples)
        .map_err(OfflineSessionError::Pool)?;
    let ctx = state
        .ctx
        .as_mut()
        .ok_or(OfflineSessionError::GraphUnavailable)?;
    ctx.update()
        .map_err(|error| OfflineSessionError::Graph(format!("{error:?}")))?;
    ctx.active_backend_mut()
        .ok_or(OfflineSessionError::BackendUnavailable)?
        .render(
            usize::try_from(frames).map_err(|_| OfflineSessionError::TimelineOverflow)?,
            &mut output,
        )?;
    Ok(output)
}

#[derive(Debug, Error)]
pub(crate) enum OfflineSessionError {
    #[error("offline backend is unavailable")]
    BackendUnavailable,
    #[error("offline channel count cannot be represented")]
    ChannelCountOverflow,
    #[error("offline graph failed: {0}")]
    Graph(String),
    #[error("offline graph has not started")]
    GraphUnavailable,
    #[error("offline processor is unavailable")]
    ProcessorUnavailable,
    #[error("offline output pool failed: {0}")]
    Pool(kithara_bufpool::PoolError),
    #[error("offline sample count overflow")]
    SampleCountOverflow,
    #[error("offline session is gone")]
    SessionGone,
    #[error("offline timeline overflow")]
    TimelineOverflow,
}
