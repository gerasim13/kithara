use std::num::NonZeroU32;

use firewheel::{
    FirewheelContext,
    cpal::{CpalConfig, CpalStream},
};
use kithara_audio::ConsumerWakeMode;
use kithara_bufpool::HasPool;
use kithara_command::Live;
use kithara_platform::{
    sync::{Arc, Mutex, mpsc},
    thread::spawn_named,
    time::Instant,
};
use kithara_play::{SessionSampleRate, StreamShape};
use tracing::{debug, warn};

use super::{
    dispatch::{run_host_cmd, tick_session},
    protocol::{
        Cmd, HostCmd, HostCmdMsg, HostDispatchError, HostDispatcher, HostReply, Reply,
        SessionDispatcher,
    },
    queue::HostProtocol,
    state::{HostRoot, RootView, SessionState},
};
use crate::{HostSettings, consts, error::PlayError, rt::SessionOutput};

pub(crate) struct SessionClient<S> {
    cmd_tx: Mutex<mpsc::Sender<HostCmdMsg<S>>>,
    root_view: RootView,
}

impl<S> SessionClient<S> {
    fn call(&self, cmd: HostCmd<S>) -> Result<HostReply, HostDispatchError<S>> {
        let (reply_tx, reply_rx) = mpsc::channel();
        if let Err(error) = self.cmd_tx.lock().send(HostCmdMsg { cmd, reply_tx }) {
            return Err(HostDispatchError::before_send(
                PlayError::SessionGone {
                    reason: "session thread stopped accepting commands",
                },
                error.0.cmd,
            ));
        }
        let reply = reply_rx.recv().map_err(|_| {
            HostDispatchError::after_send(PlayError::SessionGone {
                reason: "session thread dropped the reply channel",
            })
        })?;
        Ok(reply)
    }
}

impl<S: Send + Sync + 'static> SessionDispatcher<S> for SessionClient<S> {
    fn consumer_wake_mode(&self) -> ConsumerWakeMode {
        ConsumerWakeMode::RealtimeDeferred
    }

    fn exec(&self, cmd: Cmd<S>) -> Result<Reply, PlayError> {
        match self.call(HostCmd::Play(cmd)).map_err(PlayError::from)? {
            HostReply::Play(reply) => Ok(reply),
            HostReply::Err(error) => Err(error),
            HostReply::Ok => Err(PlayError::Internal(
                "unexpected host reply for player session command".into(),
            )),
        }
    }

    delegate::delegate! {
        to self.root_view {
            fn sample_rate(&self) -> SessionSampleRate;
            fn stream_shape(&self) -> Option<StreamShape>;
        }
    }
}

impl<S: Send + Sync + 'static> HostDispatcher<S> for SessionClient<S> {
    fn exec_host(&self, cmd: HostCmd<S>) -> Result<HostReply, HostDispatchError<S>> {
        self.call(cmd)
    }
}

/// Disconnects queued callers, stops the stream with the session state, and
/// only then replies, so the Host drops its decks once nothing renders them.
fn complete_shutdown<T, S>(
    cmd_rx: mpsc::Receiver<HostCmdMsg<S>>,
    state: SessionState<T, S>,
    reply_tx: &mpsc::Sender<HostReply>,
) {
    drop(cmd_rx);
    drop(state);
    if reply_tx.send(HostReply::Ok).is_err() {
        warn!("[KITHARA-ROUTE] native shutdown reply receiver dropped");
    }
}

/// Waits for the next message: until `deadline` while `active`, so the caller
/// can pump on its interval, and with no deadline otherwise.
pub(crate) fn receive_message<M>(
    cmd_rx: &mpsc::Receiver<M>,
    active: bool,
    deadline: Instant,
) -> Result<Option<M>, mpsc::RecvTimeoutError> {
    if active {
        match cmd_rx.recv_timeout(deadline) {
            Ok(message) => Ok(Some(message)),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(error) => Err(error),
        }
    } else {
        cmd_rx
            .recv()
            .map(Some)
            .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
    }
}

fn service_due_tick<T, S>(state: &mut SessionState<T, S>, deadline: &mut Instant) {
    if state.ctx.is_some() && Instant::now() >= *deadline {
        if let Reply::Err(error) = tick_session(state) {
            warn!(?error, "native session tick failed");
        }
        *deadline = Instant::now() + consts::SESSION_PUMP_INTERVAL;
    }
}

fn engine_thread<T, S>(
    cmd_rx: mpsc::Receiver<HostCmdMsg<S>>,
    root: HostRoot,
    root_view: RootView,
    requested_max_block_frames: Option<NonZeroU32>,
    output: SessionOutput,
    settings: Live<HostSettings, HostProtocol>,
    start_stream_fn: impl FnMut(&mut FirewheelContext, u32) -> Result<T, String> + Send + 'static,
) where
    S: HasPool<f32> + Send + Sync + 'static,
{
    let mut state = SessionState::<T, S>::new(
        root,
        root_view,
        requested_max_block_frames,
        None,
        output,
        settings,
        start_stream_fn,
    );
    debug!("[KITHARA-ROUTE] native session worker started");
    let mut deadline = Instant::now() + consts::SESSION_PUMP_INTERVAL;
    loop {
        let Ok(message) = receive_message(&cmd_rx, state.ctx.is_some(), deadline) else {
            break;
        };
        if let Some(HostCmdMsg { cmd, reply_tx }) = message {
            if matches!(&cmd, HostCmd::Shutdown) {
                complete_shutdown(cmd_rx, state, &reply_tx);
                debug!("[KITHARA-ROUTE] native session worker stopped");
                return;
            }
            let reply = run_host_cmd(&mut state, cmd);
            if reply_tx.send(reply).is_err() {
                warn!("[KITHARA-ROUTE] native session reply receiver dropped");
            }
        }
        service_due_tick(&mut state, &mut deadline);
    }
    debug!("[KITHARA-ROUTE] native session worker stopped");
}

fn spawn_session_client<T, S>(
    thread_name: &'static str,
    root: HostRoot,
    root_view: RootView,
    requested_max_block_frames: Option<NonZeroU32>,
    output: SessionOutput,
    settings: Live<HostSettings, HostProtocol>,
    start_stream_fn: impl FnMut(&mut FirewheelContext, u32) -> Result<T, String> + Send + 'static,
) -> Arc<SessionClient<S>>
where
    S: HasPool<f32> + Send + Sync + 'static,
{
    let (cmd_tx, cmd_rx) = mpsc::channel::<HostCmdMsg<S>>();
    let client_view = root_view.clone();
    spawn_named(thread_name, move || {
        engine_thread::<T, S>(
            cmd_rx,
            root,
            root_view,
            requested_max_block_frames,
            output,
            settings,
            start_stream_fn,
        );
    });
    Arc::new(SessionClient {
        root_view: client_view,
        cmd_tx: Mutex::new(cmd_tx),
    })
}

fn start_stream_cpal(
    ctx: &mut FirewheelContext,
    sample_rate: u32,
    output_block_frames: Option<NonZeroU32>,
) -> Result<CpalStream, String> {
    debug!(sample_rate, "[KITHARA-ROUTE] starting cpal stream");
    let config = cpal_config(sample_rate, output_block_frames);
    match CpalStream::new(ctx, config) {
        Ok(stream) => {
            debug!(sample_rate, "[KITHARA-ROUTE] cpal stream started");
            Ok(stream)
        }
        Err(err) => {
            warn!(
                sample_rate,
                ?err,
                "[KITHARA-ROUTE] cpal stream start failed"
            );
            Err(err.to_string())
        }
    }
}

fn cpal_config(sample_rate: u32, output_block_frames: Option<NonZeroU32>) -> CpalConfig {
    let mut config = CpalConfig::default();
    config.output.desired_sample_rate = NonZeroU32::new(sample_rate).map(NonZeroU32::get);
    if let Some(frames) = output_block_frames {
        config.output.desired_block_frames = Some(frames.get());
    }
    config
}

pub(crate) fn spawn<S: HasPool<f32> + Send + Sync + 'static>(
    root: HostRoot,
    root_view: RootView,
    output_block_frames: Option<NonZeroU32>,
    output: SessionOutput,
    settings: Live<HostSettings, HostProtocol>,
) -> Arc<dyn HostDispatcher<S>> {
    spawn_session_client::<CpalStream, S>(
        "kithara-engine",
        root,
        root_view,
        output_block_frames,
        output,
        settings,
        move |ctx, sample_rate| start_stream_cpal(ctx, sample_rate, output_block_frames),
    )
}

#[cfg(test)]
mod tests {
    use kithara_command::When;
    use kithara_effects::LimiterConfig;
    use kithara_events::EventBus;
    use kithara_platform::time::Duration;
    use kithara_play::DeckMixerConfig;
    use kithara_test_utils::{
        bufpool::{TestPools, pools},
        kithara, wait_until,
    };
    use kithara_warp::BeatGridState;

    use super::*;
    use crate::{
        HostSettingsChange, MetronomeConfigChange,
        session::tests::{
            graph::root_with_player,
            ring::{MasterRing, RingBackend, RingBackendConfig, RingLayout},
        },
    };

    #[kithara::test]
    fn output_block_override_preserves_the_backend_default_or_sets_128() {
        let inherited = cpal_config(44_100, None);
        assert_eq!(
            inherited.output.desired_block_frames,
            firewheel::cpal::CpalOutputConfig::default().desired_block_frames
        );

        let frames = NonZeroU32::new(128).expect("test block size is non-zero");
        let configured = cpal_config(44_100, Some(frames));
        assert_eq!(configured.output.desired_block_frames, Some(128));
    }

    #[kithara::test]
    fn idle_native_worker_publishes_rendered_transport_without_a_deck_tick() {
        let runtime = kithara_platform::tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("test wait runtime");
        let sample_rate = NonZeroU32::new(48_000).expect("test sample rate");
        let (root, root_view, player_grid_id) = root_with_player(sample_rate);
        let (writer, mut reader) = MasterRing::open(512, 4);
        let mut writer = Some(writer);
        let (stream_tx, stream_rx) = mpsc::channel();
        let client = spawn_session_client::<Arc<Mutex<RingBackend>>, TestPools>(
            "host-pump-regression",
            root,
            root_view.clone(),
            None,
            SessionOutput::new(LimiterConfig::default()),
            Live::new(HostSettings::builder().sample_rate(sample_rate).build())
                .expect("the fixture settings are valid"),
            move |ctx, _| {
                let backend = RingBackend::start(
                    ctx,
                    RingBackendConfig::new(
                        sample_rate,
                        RingLayout::Stereo,
                        writer.take().ok_or("backend already started")?,
                    ),
                )
                .map_err(|err| err.to_string())?;
                let stream = Arc::new(Mutex::new(backend));
                stream_tx
                    .send(Arc::clone(&stream))
                    .map_err(|err| err.to_string())?;
                Ok(stream)
            },
        );

        let player_id = match client.exec(Cmd::RegisterPlayer {
            grid_id: player_grid_id,
            bus: EventBus::default(),
            mixer: DeckMixerConfig::default(),
            pools: pools(),
        }) {
            Ok(Reply::PlayerRegistered(player_id)) => player_id,
            Ok(Reply::Err(error)) => panic!("register fixture player: {error}"),
            Err(error) => panic!("register fixture player: {error}"),
            _ => panic!("unexpected register fixture player reply"),
        };
        assert!(matches!(
            client.exec(Cmd::StartPlayer {
                player_id,
                render_quantum_frames: None,
                response_budget_frames: None,
            }),
            Ok(Reply::Ok)
        ));
        let stream = stream_rx.recv().expect("active graph backend");
        stream.lock().arm();
        stream.lock().render_block(0).expect("initial render");
        let _ = reader.drain(512);
        let mut clock_samples = 512;
        runtime
            .block_on(wait_until(
                Duration::from_secs(2),
                "native Host delivers graph change",
                || {
                    stream
                        .lock()
                        .render_block(clock_samples)
                        .expect("render graph");
                    clock_samples += 512;
                    let _ = reader.drain(512);
                    root_view.grid().state() == BeatGridState::Live
                },
            ))
            .expect("the transport's own tempo reaches the read-only Host view without a command");

        assert!(matches!(
            client.exec_host(HostCmd::Configure {
                change: HostSettingsChange::Metronome(MetronomeConfigChange::Enabled(true)),
                at: When::Next,
            }),
            Ok(HostReply::Ok)
        ));
        let mut on_blocks = 0;
        runtime
            .block_on(wait_until(
                Duration::from_secs(3),
                "idle native metronome sounds",
                || {
                    let mut sounded = false;
                    for _ in 0..100 {
                        stream
                            .lock()
                            .render_block(clock_samples)
                            .expect("render metronome");
                        clock_samples += 512;
                        on_blocks += 1;
                        sounded |= reader.drain(512).iter().any(|sample| sample.abs() > 0.01);
                    }
                    sounded
                },
            ))
            .unwrap_or_else(|error| {
                panic!("metronome on reaches PCM: {error}; blocks={on_blocks}")
            });

        assert!(matches!(
            client.exec_host(HostCmd::Configure {
                change: HostSettingsChange::Metronome(MetronomeConfigChange::Enabled(false)),
                at: When::Next,
            }),
            Ok(HostReply::Ok)
        ));
        runtime
            .block_on(wait_until(
                Duration::from_secs(4),
                "idle native metronome stays off",
                || {
                    let mut silent = true;
                    for _ in 0..100 {
                        stream
                            .lock()
                            .render_block(clock_samples)
                            .expect("render after metronome off");
                        clock_samples += 512;
                        let pcm = reader.drain(512);
                        silent &= pcm.iter().all(|sample| sample.abs() < 1e-6);
                    }
                    silent
                },
            ))
            .expect("the muted capture spans more than one beat");
        assert!(matches!(
            client.exec_host(HostCmd::Shutdown),
            Ok(HostReply::Ok)
        ));
    }
}
