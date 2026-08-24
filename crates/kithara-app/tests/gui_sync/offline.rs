use std::{error::Error, num::NonZeroU32};

use ::kithara::{
    audio::ConsumerWakeMode,
    play::{Cmd, PlayError, Reply, SessionDispatcher, SessionState, run_cmd},
};
use firewheel::{
    FirewheelCtx, StreamInfo,
    backend::{AudioBackend, BackendProcessInfo},
    node::StreamStatus,
    processor::FirewheelProcessor,
};
use kithara_platform::{
    sync::{Mutex, mpsc},
    thread::{JoinHandle, spawn_named},
    time::{self, Duration},
};
use num_traits::cast::AsPrimitive;

pub(super) const BLOCK_FRAMES: usize = 512;
pub(super) const CHANNELS: u16 = 2;
pub(super) const SAMPLE_RATE: u32 = 44_100;

enum OfflineMsg {
    Cmd {
        cmd: Cmd,
        reply_tx: mpsc::Sender<Reply>,
    },
    Render {
        frames: usize,
        reply_tx: mpsc::Sender<Vec<f32>>,
    },
    Shutdown,
}

pub(super) struct OfflineSession {
    cmd_tx: Mutex<mpsc::Sender<OfflineMsg>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl Default for OfflineSession {
    fn default() -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let worker = spawn_named("kithara-app-sync-offline", move || {
            let mut state = SessionState::<OfflineBackend>::new(start_stream_offline);
            for message in cmd_rx.iter() {
                match message {
                    OfflineMsg::Cmd { cmd, reply_tx } => {
                        let _ = reply_tx.send(run_cmd(&mut state, cmd));
                    }
                    OfflineMsg::Render { frames, reply_tx } => {
                        let _ = reply_tx.send(render_offline(&mut state, frames));
                    }
                    OfflineMsg::Shutdown => break,
                }
            }
        });
        Self {
            cmd_tx: Mutex::new(cmd_tx),
            worker: Mutex::new(Some(worker)),
        }
    }
}

impl OfflineSession {
    #[kithara::allow_block]
    pub(super) fn render(&self, frames: usize) -> Vec<f32> {
        let (reply_tx, reply_rx) = mpsc::channel();
        if self
            .cmd_tx
            .lock()
            .send(OfflineMsg::Render { frames, reply_tx })
            .is_err()
        {
            return Vec::new();
        }
        reply_rx.recv().unwrap_or_default()
    }
}

impl Drop for OfflineSession {
    fn drop(&mut self) {
        let _ = self.cmd_tx.lock().send(OfflineMsg::Shutdown);
        if let Some(worker) = self.worker.lock().take() {
            let _ = worker.join();
        }
    }
}

impl SessionDispatcher for OfflineSession {
    fn consumer_wake_mode(&self) -> ConsumerWakeMode {
        ConsumerWakeMode::ImmediateOffRt
    }

    #[kithara::allow_block]
    fn exec(&self, cmd: Cmd) -> Result<Reply, PlayError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.cmd_tx
            .lock()
            .send(OfflineMsg::Cmd { cmd, reply_tx })
            .map_err(|_| PlayError::Internal("offline app session stopped".into()))?;
        reply_rx
            .recv()
            .map_err(|_| PlayError::Internal("offline app session reply closed".into()))
    }
}

struct OfflineBackend {
    frames_rendered: u64,
    processor: Option<FirewheelProcessor<Self>>,
    sample_rate: u32,
}

#[derive(Clone)]
struct OfflineConfig {
    block_frames: u32,
    sample_rate: u32,
}

impl Default for OfflineConfig {
    fn default() -> Self {
        Self {
            block_frames: u32::try_from(BLOCK_FRAMES).expect("block size fits u32"),
            sample_rate: SAMPLE_RATE,
        }
    }
}

#[derive(Debug, derive_more::Display)]
#[display("offline backend error")]
struct OfflineError;

impl Error for OfflineError {}

impl AudioBackend for OfflineBackend {
    type Config = OfflineConfig;
    type Enumerator = ();
    type Instant = time::Instant;
    type StartStreamError = OfflineError;
    type StreamError = OfflineError;

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
        let sample_rate = NonZeroU32::new(config.sample_rate).expect("non-zero sample rate");
        let block_frames = NonZeroU32::new(config.block_frames).expect("non-zero block size");
        let info = StreamInfo {
            sample_rate,
            sample_rate_recip: 1.0 / f64::from(config.sample_rate),
            prev_sample_rate: sample_rate,
            max_block_frames: block_frames,
            num_stream_in_channels: 0,
            num_stream_out_channels: u32::from(CHANNELS),
            input_to_output_latency_seconds: 0.0,
            declick_frames: block_frames,
            output_device_id: "offline-app-sync".to_owned(),
            input_device_id: None,
        };
        Ok((
            Self {
                frames_rendered: 0,
                processor: None,
                sample_rate: config.sample_rate,
            },
            info,
        ))
    }
}

impl OfflineBackend {
    fn render(&mut self, frames: usize) -> Vec<f32> {
        let mut output = vec![0.0; frames.saturating_mul(usize::from(CHANNELS))];
        if let Some(processor) = &mut self.processor {
            let rendered: f64 = self.frames_rendered.as_();
            let info = BackendProcessInfo {
                frames,
                num_in_channels: 0,
                num_out_channels: usize::from(CHANNELS),
                process_timestamp: time::Instant::now(),
                duration_since_stream_start: Duration::from_secs_f64(
                    rendered / f64::from(self.sample_rate),
                ),
                input_stream_status: StreamStatus::empty(),
                output_stream_status: StreamStatus::empty(),
                dropped_frames: 0,
            };
            processor.process_interleaved(&[], &mut output, info);
        }
        self.frames_rendered = self
            .frames_rendered
            .saturating_add(u64::try_from(frames).unwrap_or(u64::MAX));
        output
    }
}

fn start_stream_offline(
    context: &mut FirewheelCtx<OfflineBackend>,
    sample_rate: u32,
) -> Result<(), String> {
    context
        .start_stream(OfflineConfig {
            block_frames: u32::try_from(BLOCK_FRAMES).expect("block size fits u32"),
            sample_rate,
        })
        .map_err(|error| error.to_string())
}

fn render_offline(state: &mut SessionState<OfflineBackend>, frames: usize) -> Vec<f32> {
    let Some(context) = state.ctx_mut() else {
        return Vec::new();
    };
    if context.update().is_err() {
        return Vec::new();
    }
    context
        .active_backend_mut()
        .map_or_else(Vec::new, |backend| backend.render(frames))
}
