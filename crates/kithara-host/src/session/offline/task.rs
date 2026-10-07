use kithara_bufpool::{HasPool, PoolRegion};
use kithara_command::{Live, mailbox};
use kithara_config::Config;
use kithara_platform::{
    maybe_send::MaybeSend,
    sync::{Arc, mpsc, mpsc::TryRecvError},
    time::Duration,
};
use kithara_play::PlayError;
use kithara_worker::{Dispatcher, Task, TaskConfig, TaskHandle, TickResult};
use std::{marker::PhantomData, num::NonZeroU32};
use thiserror::Error;

use super::{
    OfflineSessionClient,
    backend::{BackendConfig, OfflineStream},
};
use crate::{
    HostCore, HostOwner, HostSettings,
    rt::SessionOutput,
    session::{
        decks::{DeckInbox, DeckMsg},
        dispatch::OwnerPosts,
        protocol::HostMailbox,
        queue::HostProtocol,
        state::{HostRoot, RootView, SessionState, SessionStream},
    },
};

pub(crate) mod consts {
    pub(crate) const CHANNELS: usize = 2;
}

pub(super) enum OfflineMsg {
    Posted,
    Deck(DeckMsg),
    Shutdown,
}

struct OfflineSessionTask<S, O: HostOwner<S>> {
    cmd_rx: mpsc::Receiver<OfflineMsg>,
    mailbox: HostMailbox<O::Command>,
    owner: O,
    posts: OwnerPosts,
    marker: PhantomData<fn() -> S>,
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

impl<S, O: HostOwner<S>> Task for OfflineSessionTask<S, O> {
    fn tick(&mut self) -> TickResult {
        match self.cmd_rx.try_recv() {
            Ok(OfflineMsg::Posted) => {
                self.posts.drain(&mut self.owner, &mut self.mailbox);
                self.posts.pass(&mut self.owner);
                TickResult::Progress
            }
            Ok(OfflineMsg::Deck(message)) => {
                message.run(&mut self.owner);
                TickResult::Progress
            }
            Ok(OfflineMsg::Shutdown) | Err(TryRecvError::Disconnected) => TickResult::Done,
            Err(TryRecvError::Empty) => TickResult::Waiting,
            #[cfg(target_arch = "wasm32")]
            Err(_) => TickResult::Waiting,
        }
    }
}

pub(crate) fn spawn<S, O>(
    dispatcher: &Dispatcher,
    task_config: TaskConfig,
    root: HostRoot,
    root_view: RootView,
    config: OfflineTaskConfig<S>,
    layer: impl FnOnce(HostCore<S, O::Deck>) -> O + MaybeSend + 'static,
) -> Result<(Arc<OfflineSessionClient<O::Command>>, TaskHandle), PlayError>
where
    S: HasPool<f32> + Send + Sync + 'static,
    O: HostOwner<S>,
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
    let (postbox, mailbox) = mailbox();
    let pending = dispatcher.reserve(task_config).map_err(|error| {
        PlayError::Internal(format!("offline session task reservation: {error}"))
    })?;
    let client = Arc::new(OfflineSessionClient::new(
        postbox,
        cmd_tx,
        pending.context().control(),
    ));
    let inbox: Arc<dyn DeckInbox> = client.clone();
    let task = pending
        .start_local(move |_| {
            let start = move |ctx: &mut firewheel::FirewheelContext, rate: u32| {
                let rate = NonZeroU32::new(rate)
                    .ok_or_else(|| "offline sample rate must be non-zero".to_owned())?;
                let backend = BackendConfig::builder()
                    .block_frames(max_block_frames)
                    .declared_latency(declared_latency)
                    .sample_rate(rate)
                    .build();
                OfflineStream::start(ctx, backend)
                    .map(SessionStream::Offline)
                    .map_err(|error| error.to_string())
            };
            let state = SessionState::new(
                root,
                root_view,
                Some(max_block_frames),
                Some(declick_frames),
                output,
                settings,
                start,
            );
            let _ = pools;
            OfflineSessionTask {
                cmd_rx,
                mailbox,
                owner: layer(HostCore::new(state, inbox)),
                posts: OwnerPosts::new(),
                marker: PhantomData,
            }
        })
        .map_err(|error| PlayError::Internal(format!("offline session task start: {error}")))?;
    Ok((client, task))
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
