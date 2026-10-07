use std::num::NonZeroU32;

use kithara_bufpool::{HasPool, PoolRegion, SampleBuffer};
use kithara_command::Live;
use kithara_config::Config;
use kithara_platform::{
    sync::{Arc, mpsc, mpsc::TryRecvError},
    time::Duration,
};
use kithara_play::PlayError;
use kithara_worker::{Dispatcher, Task, TaskConfig, TaskHandle, TickResult};
use thiserror::Error;
use tracing::warn;

#[cfg(not(target_arch = "wasm32"))]
use super::super::decks::{DeckMsg, Decks};
use super::{
    super::{
        dispatch::run_host_cmd,
        protocol::{HostCmd, answer},
        queue::{HostProtocol, settle_receipts},
        state::{HostRoot, RootView, SessionState, ensure_ctx},
        transport,
    },
    OfflineSessionClient,
    backend::{BackendConfig, OfflineStream},
};
use crate::{HostSettings, rt::SessionOutput};

pub(crate) mod consts {
    pub(crate) const CHANNELS: usize = 2;
}

pub(super) enum OfflineMsg<S> {
    Host(HostCmd<S>),
    #[cfg(not(target_arch = "wasm32"))]
    Deck(DeckMsg),
    Position {
        reply_tx: mpsc::Sender<u64>,
    },
    Render {
        position: u64,
        frames: u32,
        reply_tx: mpsc::Sender<Result<SampleBuffer, OfflineSessionError>>,
    },
}

struct OfflineSessionTask<S> {
    max_block_frames: NonZeroU32,
    cmd_rx: Option<mpsc::Receiver<OfflineMsg<S>>>,
    state: Option<SessionState<OfflineStream, S>>,
    /// The Host's decks; on the web they stay on the Host's Worker.
    #[cfg(not(target_arch = "wasm32"))]
    decks: Decks,
    pools: PoolRegion<S>,
    position: u64,
}

#[derive(Config)]
#[config(construction)]
pub(crate) struct OfflineTaskConfig<S> {
    #[config(skip = "applied to the offline backend")]
    pub(crate) declared_latency: Duration,
    #[config(skip = "transferred to session output")]
    pub(crate) output: SessionOutput,
    #[config(skip = "transferred to session state")]
    pub(crate) settings: Live<HostSettings, HostProtocol>,
    #[config(skip = "transferred to session state")]
    pub(crate) declick_frames: NonZeroU32,
    #[config(skip = "transferred to the offline task")]
    pub(crate) max_block_frames: NonZeroU32,
    #[config(skip = "transferred to the offline task")]
    pub(crate) pools: PoolRegion<S>,
}

impl<S> OfflineSessionTask<S>
where
    S: HasPool<f32> + Send + Sync + 'static,
{
    fn render(&mut self, position: u64, frames: u32) -> Result<SampleBuffer, OfflineSessionError> {
        if position != self.position {
            return Err(OfflineSessionError::CursorChanged {
                expected: position,
                actual: self.position,
            });
        }
        if frames == 0 || frames > self.max_block_frames.get() {
            return Err(OfflineSessionError::InvalidBlockFrames {
                requested: frames,
                maximum: self.max_block_frames.get(),
            });
        }
        let state = self
            .state
            .as_mut()
            .ok_or(OfflineSessionError::SessionGone)?;
        let output = render_block(state, frames, self.position, &self.pools)?;
        self.position = self
            .position
            .checked_add(u64::from(frames))
            .ok_or(OfflineSessionError::TimelineOverflow)?;
        Ok(output)
    }

    /// Runs one Host command. The session state goes only with the channel on
    /// shutdown or with the task on cancel, so a command never finds it gone.
    fn tick_host(&mut self, cmd: HostCmd<S>) -> TickResult {
        if let HostCmd::Shutdown(reply) = cmd {
            drop(self.cmd_rx.take());
            self.stop();
            answer(&reply, ());
            return TickResult::Done;
        }
        let Some(state) = self.state.as_mut() else {
            return TickResult::Done;
        };
        run_host_cmd(state, cmd);
        TickResult::Progress
    }

    /// Stops the stream with the session state, then drops the decks it
    /// rendered, each released first: only the session task lets go of them.
    fn stop(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        self.decks.release_all();
        self.state.take();
        #[cfg(not(target_arch = "wasm32"))]
        drop(std::mem::take(&mut self.decks));
    }

    fn tick_message(&mut self, message: OfflineMsg<S>) -> TickResult {
        match message {
            OfflineMsg::Host(message) => self.tick_host(message),
            #[cfg(not(target_arch = "wasm32"))]
            OfflineMsg::Deck(message) => {
                self.decks.run(message);
                TickResult::Progress
            }
            OfflineMsg::Position { reply_tx } => self.tick_position(&reply_tx),
            OfflineMsg::Render {
                position,
                frames,
                reply_tx,
            } => self.tick_render(position, frames, &reply_tx),
        }
    }

    fn tick_position(&self, reply_tx: &mpsc::Sender<u64>) -> TickResult {
        if reply_tx.send(self.position).is_err() {
            warn!("offline position reply receiver dropped");
        }
        TickResult::Progress
    }

    fn tick_render(
        &mut self,
        position: u64,
        frames: u32,
        reply_tx: &mpsc::Sender<Result<SampleBuffer, OfflineSessionError>>,
    ) -> TickResult {
        let reply = self.render(position, frames);
        if reply_tx.send(reply).is_err() {
            warn!("offline render reply receiver dropped");
        }
        TickResult::Progress
    }
}

impl<S> Task for OfflineSessionTask<S>
where
    S: HasPool<f32> + Send + Sync + 'static,
{
    fn on_cancel(&mut self) {
        self.stop();
    }

    fn tick(&mut self) -> TickResult {
        let Some(cmd_rx) = self.cmd_rx.as_ref() else {
            return TickResult::Done;
        };
        match cmd_rx.try_recv() {
            Ok(message) => self.tick_message(message),
            Err(TryRecvError::Disconnected) => TickResult::Done,
            Err(TryRecvError::Empty) => TickResult::Waiting,
            #[cfg(target_arch = "wasm32")]
            Err(_) => TickResult::Waiting,
        }
    }
}

pub(crate) fn spawn<S>(
    dispatcher: &Dispatcher,
    task_config: TaskConfig,
    root: HostRoot,
    root_view: RootView,
    config: OfflineTaskConfig<S>,
) -> Result<(Arc<OfflineSessionClient<S>>, TaskHandle), PlayError>
where
    S: HasPool<f32> + Send + Sync + 'static,
{
    let OfflineTaskConfig {
        pools,
        max_block_frames,
        declick_frames,
        declared_latency,
        output,
        settings,
    } = config;
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let pending = dispatcher.reserve(task_config).map_err(|error| {
        PlayError::Internal(format!("offline session task reservation: {error}"))
    })?;
    let control = pending.context().control();
    let client = Arc::new(OfflineSessionClient::new(cmd_tx, control));
    let task = pending
        .start_local(move |_| {
            let start_stream = move |ctx: &mut firewheel::FirewheelContext, rate: u32| {
                let rate = NonZeroU32::new(rate)
                    .ok_or_else(|| "offline sample rate must be non-zero".to_owned())?;
                let config = BackendConfig::builder()
                    .block_frames(max_block_frames)
                    .declared_latency(declared_latency)
                    .sample_rate(rate)
                    .build();
                OfflineStream::start(ctx, config).map_err(|error| error.to_string())
            };
            OfflineSessionTask {
                cmd_rx: Some(cmd_rx),
                max_block_frames,
                pools,
                position: 0,
                #[cfg(not(target_arch = "wasm32"))]
                decks: Decks::default(),
                state: Some(SessionState::new(
                    root,
                    root_view,
                    Some(max_block_frames),
                    Some(declick_frames),
                    output,
                    settings,
                    start_stream,
                )),
            }
        })
        .map_err(|error| PlayError::Internal(format!("offline session task start: {error}")))?;
    Ok((client, task))
}

fn render_block<S>(
    state: &mut SessionState<OfflineStream, S>,
    frames: u32,
    position: u64,
    pools: &PoolRegion<S>,
) -> Result<SampleBuffer, OfflineSessionError>
where
    S: HasPool<f32> + Send + Sync + 'static,
{
    if state.ctx.is_none() {
        ensure_ctx(state).map_err(|error| OfflineSessionError::Graph(error.to_string()))?;
    }
    let total_samples = usize::try_from(frames)
        .map_err(|_| OfflineSessionError::SampleCountOverflow)?
        .checked_mul(consts::CHANNELS)
        .ok_or(OfflineSessionError::SampleCountOverflow)?;
    let mut output = pools
        .get_with_len::<f32>(total_samples)
        .map_err(OfflineSessionError::Pool)?;
    state
        .ctx
        .as_mut()
        .ok_or(OfflineSessionError::GraphUnavailable)?
        .update()
        .map_err(|error| OfflineSessionError::Graph(format!("{error:?}")))?;
    state
        .stream
        .as_mut()
        .ok_or(OfflineSessionError::BackendUnavailable)?
        .render(
            position,
            usize::try_from(frames).map_err(|_| OfflineSessionError::TimelineOverflow)?,
            &mut output,
        )?;
    transport::observe_commits(state);
    settle_receipts(state);
    Ok(output)
}

#[derive(Debug, Error)]
pub(crate) enum OfflineSessionError {
    #[error("offline backend is unavailable")]
    BackendUnavailable,
    #[error("offline channel count cannot be represented")]
    ChannelCountOverflow,
    #[error("offline render expected cursor {expected}, but the session is at {actual}")]
    CursorChanged { expected: u64, actual: u64 },
    #[error("offline graph failed: {0}")]
    Graph(String),
    #[error("offline graph has not started")]
    GraphUnavailable,
    #[error("offline block requests {requested} frames, maximum is {maximum}")]
    InvalidBlockFrames { requested: u32, maximum: u32 },
    #[error("offline output pool failed: {0}")]
    Pool(kithara_bufpool::PoolError),
    #[error("offline sample count overflow")]
    SampleCountOverflow,
    #[error("offline session is gone")]
    SessionGone,
    #[error("offline timeline overflow")]
    TimelineOverflow,
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::num::NonZeroUsize;

    use kithara_effects::LimiterConfig;
    use kithara_platform::thread::sleep;
    use kithara_test_utils::{
        bufpool::{TestPools, pools},
        kithara,
    };
    use kithara_worker::{DispatcherConfig, Worker, WorkerConfig};

    use super::*;
    use crate::{
        consts::{self, SESSION_PUMP_INTERVAL},
        session::{
            decks::SessionDecks,
            protocol::ask,
            tests::{
                deck_probe::{Seen, next, probe, so_far, ticks},
                graph::empty_root,
            },
        },
    };

    /// An offline session with no graph that holds probe decks, and the
    /// worker it runs on.
    struct DeckSession {
        client: Arc<OfflineSessionClient<TestPools>>,
        decks: SessionDecks,
        _task: TaskHandle,
        _dispatcher: Dispatcher,
        _worker: Worker,
    }

    impl DeckSession {
        fn spawn() -> Self {
            let sample_rate = NonZeroU32::new(48_000).expect("test sample rate");
            let block = NonZeroU32::new(512).expect("test block");
            let (root, root_view) = empty_root(sample_rate);
            let worker = Worker::new(WorkerConfig::new());
            let dispatcher = worker.dispatcher(
                DispatcherConfig::builder()
                    .name(consts::DECK_SESSION)
                    .capacity(NonZeroUsize::MIN)
                    .build(),
            );
            let (client, task) = spawn(
                &dispatcher,
                TaskConfig::new(),
                root,
                root_view,
                OfflineTaskConfig::builder()
                    .declared_latency(Duration::ZERO)
                    .output(SessionOutput::new(LimiterConfig::default()))
                    .settings(
                        Live::new(HostSettings::builder().sample_rate(sample_rate).build())
                            .expect("the fixture settings are valid"),
                    )
                    .declick_frames(block)
                    .max_block_frames(block)
                    .pools(pools())
                    .build(),
            )
            .expect("the offline session starts");
            let decks = SessionDecks::new(client.clone());
            Self {
                client,
                decks,
                _task: task,
                _dispatcher: dispatcher,
                _worker: worker,
            }
        }

        /// Returns once the session ran everything sent to it before.
        fn settle(&self) {
            self.client.position().expect("the session answers");
        }
    }

    impl Drop for DeckSession {
        fn drop(&mut self) {
            assert!(ask(&*self.client, HostCmd::Shutdown).is_ok());
        }
    }

    #[kithara::test]
    fn an_offline_session_ticks_its_decks_only_ahead_of_a_block() {
        let session = DeckSession::spawn();
        let (id, deck, seen) = probe();

        session
            .decks
            .hold(id, deck)
            .expect("the session takes the deck");
        sleep(SESSION_PUMP_INTERVAL * 3);
        session.settle();
        assert_eq!(ticks(&so_far(&seen)), 0, "no clock ticks an offline deck");

        session.decks.tick_block();
        session.settle();
        let ticked = so_far(&seen);
        assert_eq!(ticks(&ticked), 1, "one tick ahead of the block");
        assert!(
            ticked.iter().any(
                |seen| matches!(seen, Seen::Ticked(thread) if thread.as_deref() == Some(consts::DECK_SESSION))
            ),
            "the session task ticks its decks"
        );
    }

    #[kithara::test]
    fn an_offline_deck_drains_on_its_wake_with_no_block_rendered() {
        let session = DeckSession::spawn();
        let (id, deck, seen) = probe();
        session
            .decks
            .hold(id, deck)
            .expect("the session takes the deck");
        let Seen::Held(waker) = next(&seen) else {
            panic!("the session holds the deck before anything else");
        };
        assert!(
            matches!(next(&seen), Seen::Drained(_)),
            "drained as it is held"
        );

        waker.wake_by_ref();

        assert!(
            matches!(next(&seen), Seen::Drained(thread) if thread.as_deref() == Some(consts::DECK_SESSION)),
            "a woken deck runs its commands on the session task before any tick"
        );
    }
}
