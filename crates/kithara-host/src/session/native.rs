use std::{
    num::NonZeroU32,
    task::{Wake, Waker},
};

use firewheel::{
    FirewheelContext,
    cpal::{CpalConfig, CpalStream},
};
use kithara_bufpool::HasPool;
use kithara_command::{Live, Ticket, mailbox};
use kithara_platform::{
    maybe_send::MaybeSend,
    sync::{Arc, Mutex, mpsc},
    thread::spawn_named,
    time::Instant,
};
use tracing::debug;

use super::{
    decks::{DeckInbox, DeckMsg},
    dispatch::OwnerPosts,
    protocol::{HostDispatchError, HostDispatcher, HostMailbox, HostPostbox, not_taken},
    queue::HostProtocol,
    state::{HostRoot, RootView, SessionBufferConfig, SessionState, SessionStream},
};
use crate::{HostCore, HostOwner, HostSettings, PlayError, consts, rt::SessionOutput};

pub(crate) enum EngineMsg {
    Posted,
    Deck(DeckMsg),
    Shutdown,
}

pub(crate) struct SessionClient<C> {
    postbox: HostPostbox<C>,
    cmd_tx: Mutex<mpsc::Sender<EngineMsg>>,
}

impl<C: Send + 'static> HostDispatcher<C> for SessionClient<C> {
    fn dispatch(&self, command: C) -> Result<Ticket<PlayError>, HostDispatchError> {
        let ticket = self.postbox.post(command).map_err(not_taken)?;
        Ok(ticket)
    }
    fn shutdown(&self) {
        drop(self.cmd_tx.lock().send(EngineMsg::Shutdown));
    }
}

impl<C: Send + 'static> DeckInbox for SessionClient<C> {
    fn post(&self, message: DeckMsg) -> Result<(), PlayError> {
        self.cmd_tx
            .lock()
            .send(EngineMsg::Deck(message))
            .map_err(|_| PlayError::SessionGone {
                reason: "session thread stopped accepting deck wakes",
            })
    }
}

struct SessionWake(Mutex<mpsc::Sender<EngineMsg>>);
impl Wake for SessionWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        drop(self.0.lock().send(EngineMsg::Posted));
    }
}

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

fn engine_thread<S, O: HostOwner<S>>(
    cmd_rx: &mpsc::Receiver<EngineMsg>,
    mut mailbox: HostMailbox<O::Command>,
    mut owner: O,
) {
    let mut posts = OwnerPosts::new();
    let mut deadline = Instant::now() + consts::SESSION_PUMP_INTERVAL;
    while let Ok(message) = receive_message(cmd_rx, true, deadline) {
        owner.begin_pass();
        let mut next = message;
        let mut shutdown = false;
        loop {
            match next {
                Some(EngineMsg::Posted) => posts.drain(&mut owner, &mut mailbox),
                Some(EngineMsg::Deck(message)) => message.run(&mut owner),
                Some(EngineMsg::Shutdown) => {
                    shutdown = true;
                    break;
                }
                None => {}
            }
            match cmd_rx.try_recv() {
                Ok(message) => next = Some(message),
                Err(_) => break,
            }
        }
        posts.drain(&mut owner, &mut mailbox);
        posts.pass(&mut owner, true);
        if shutdown {
            break;
        }
        deadline = Instant::now() + consts::SESSION_PUMP_INTERVAL;
    }
}

pub(crate) fn spawn<S, O>(
    root: HostRoot,
    view: RootView,
    output_block_frames: Option<NonZeroU32>,
    channel_config: kithara_command::ScopedConfig,
    output: SessionOutput,
    settings: Live<HostSettings, HostProtocol>,
    layer: impl FnOnce(HostCore<S, O::Deck>) -> O + MaybeSend + 'static,
) -> Arc<SessionClient<O::Command>>
where
    S: HasPool<f32> + Send + Sync + 'static,
    O: HostOwner<S>,
{
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let (postbox, mut mailbox) = mailbox();
    mailbox.hold(Waker::from(Arc::new(SessionWake(Mutex::new(
        cmd_tx.clone(),
    )))));
    let client = Arc::new(SessionClient {
        postbox,
        cmd_tx: Mutex::new(cmd_tx),
    });
    let inbox: Arc<dyn DeckInbox> = client.clone();
    spawn_named("host-deck-session", move || {
        let start = move |ctx: &mut FirewheelContext, rate| {
            start_stream_cpal(ctx, rate, output_block_frames)
                .map(|backend| SessionStream::Realtime { _backend: backend })
        };
        let mut state = SessionState::new(
            root,
            view,
            SessionBufferConfig {
                max_block_frames: output_block_frames,
                declick_frames: None,
            },
            output,
            settings,
            channel_config,
            start,
        );
        state.delivery_delay = Some(consts::SESSION_PUMP_INTERVAL);
        engine_thread(&cmd_rx, mailbox, layer(HostCore::new(state, inbox)));
    });
    client
}

fn start_stream_cpal(
    ctx: &mut FirewheelContext,
    sample_rate: u32,
    output_block_frames: Option<NonZeroU32>,
) -> Result<CpalStream, String> {
    debug!(sample_rate, "starting cpal stream");
    CpalStream::new(ctx, cpal_config(sample_rate, output_block_frames))
        .map_err(|error| error.to_string())
}

fn cpal_config(sample_rate: u32, output_block_frames: Option<NonZeroU32>) -> CpalConfig {
    let mut config = CpalConfig::default();
    config.output.desired_sample_rate = NonZeroU32::new(sample_rate).map(NonZeroU32::get);
    if let Some(frames) = output_block_frames {
        config.output.desired_block_frames = Some(frames.get());
    }
    config
}

#[cfg(test)]
mod tests {
    use kithara_command::{Seq, When};
    use kithara_effects::LimiterConfig;
    use kithara_platform::{thread::JoinHandle, time::Duration};
    use kithara_play::{DeckPass, HostedDeck, Outbox};
    use kithara_signal::{FrameCount, SessionFrame};
    use kithara_test_utils::{bufpool::TestPools, kithara, wait_until};
    use kithara_warp::BeatGridState;

    use super::*;
    use crate::{
        DeckId, HostCommand, HostSettingsChange, HostSettingsExec, HostSettled,
        MetronomeConfigChange,
        session::{
            protocol::ask as dispatch,
            tests::{
                deck_probe::{Seen, next, probe, so_far, ticks},
                graph::empty_root,
            },
        },
    };

    type BaseCommand = HostCommand<TestPools, dyn HostedDeck<TestPools>>;
    enum Command {
        Host(BaseCommand),
        Render {
            position: u64,
            reply: mpsc::Sender<Vec<f32>>,
        },
    }
    impl From<BaseCommand> for Command {
        fn from(command: BaseCommand) -> Self {
            Self::Host(command)
        }
    }
    struct RigOwner(HostCore<TestPools>);
    impl HostSettingsExec<()> for RigOwner {
        type At = When<SessionFrame>;
        type Output = Result<Option<Seq>, PlayError>;

        delegate::delegate! {
            to self.0 {
                fn exec_sample_rate(
                    &mut self,
                    value: NonZeroU32,
                    at: Self::At,
                    cx: &mut (),
                ) -> Self::Output;
                fn exec_tempo(
                    &mut self,
                    value: crate::api::Tempo,
                    at: Self::At,
                    cx: &mut (),
                ) -> Self::Output;
                fn exec_live(
                    &mut self,
                    change: HostSettingsChange,
                    at: Self::At,
                    cx: &mut (),
                ) -> Self::Output;
            }
        }
    }
    impl HostOwner<TestPools> for RigOwner {
        type Command = Command;
        type Deck = dyn HostedDeck<TestPools>;
        fn apply(&mut self, command: Command) -> Result<Option<Seq>, PlayError> {
            match command {
                Command::Host(command) => self.0.apply(command),
                Command::Render { position, reply } => {
                    self.0.prepare_offline()?;
                    let mut output = vec![0.0; 1_024];
                    self.0.render_offline(position, 512, &mut output)?;
                    reply.send(output).map_err(|_| PlayError::Closed)?;
                    Ok(None)
                }
            }
        }

        delegate::delegate! {
            to self.0 {
                fn register(&mut self, id: DeckId, deck: Box<Self::Deck>) -> Result<(), PlayError>;
                fn each_deck(
                    &mut self,
                    visit: &mut dyn FnMut(
                        DeckId,
                        &mut Self::Deck,
                        &mut Outbox<'_, TestPools>,
                        DeckPass<'_>,
                    ),
                );
                fn with_deck(
                    &mut self,
                    id: DeckId,
                    visit: &mut dyn FnMut(&mut Self::Deck, &mut Outbox<'_, TestPools>, DeckPass<'_>),
                ) -> Result<(), PlayError>;
                fn prepare_offline(&mut self) -> Result<(), PlayError>;
                fn render_offline(
                    &mut self,
                    position: u64,
                    frames: usize,
                    output: &mut [f32],
                ) -> Result<(), PlayError>;
                fn transport(&mut self) -> Option<crate::api::SessionTransportSnapshot>;
                fn begin_pass(&mut self);
                fn pass(&mut self) -> Vec<HostSettled>;
                fn clock(&self) -> Option<(SessionFrame, FrameCount)>;
                fn host_room(&self) -> usize;
            }
        }

        fn release_id(command: &Command) -> Option<DeckId> {
            match command {
                Command::Host(command) => HostCore::<TestPools>::release_id(command),
                Command::Render { .. } => None,
            }
        }
        fn is_next_tempo(command: &Command) -> bool {
            match command {
                Command::Host(command) => HostCore::<TestPools>::is_next_tempo(command),
                Command::Render { .. } => false,
            }
        }
    }

    fn deck_session() -> (Arc<SessionClient<Command>>, JoinHandle<()>, RootView) {
        let sample_rate = NonZeroU32::new(48_000).expect("test sample rate");
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (postbox, mut mailbox) = mailbox();
        mailbox.hold(Waker::from(Arc::new(SessionWake(Mutex::new(
            cmd_tx.clone(),
        )))));
        let client = Arc::new(SessionClient {
            postbox,
            cmd_tx: Mutex::new(cmd_tx),
        });
        let inbox: Arc<dyn DeckInbox> = client.clone();
        let (ready, started) = mpsc::channel();
        let worker = spawn_named(consts::DECK_SESSION, move || {
            let (root, view) = empty_root(sample_rate);
            let state = SessionState::new(
                root,
                view.clone(),
                SessionBufferConfig::default(),
                SessionOutput::new(LimiterConfig::default()),
                Live::new(HostSettings::builder().sample_rate(sample_rate).build())
                    .expect("fixture settings"),
                crate::HostConfig::<TestPools>::builder()
                    .build()
                    .channel_config(),
                crate::session::offline::mock::start,
            );
            ready.send(view).expect("owner thread publishes its view");
            engine_thread(&cmd_rx, mailbox, RigOwner(HostCore::new(state, inbox)));
        });
        let view = started.recv().expect("owner thread starts");
        (client, worker, view)
    }

    fn shut_down(client: &SessionClient<Command>, worker: JoinHandle<()>) {
        client.shutdown();
        assert!(worker.join().is_ok());
    }

    fn ask(client: &SessionClient<Command>, command: Command) -> Result<(), &'static str> {
        dispatch(client, command).map_err(|_| "native command refused")
    }

    fn render(client: &SessionClient<Command>, position: u64) -> Vec<f32> {
        let (reply, receiver) = mpsc::channel();
        ask(client, Command::Render { position, reply }).expect("native owner renders");
        receiver.recv().expect("rendered block")
    }

    /// Commands posted before the deck was held woke no one, so the session
    /// thread runs them as soon as it holds the deck.
    #[kithara::test]
    fn the_session_thread_drains_a_deck_as_it_takes_it() {
        let (client, worker, _view) = deck_session();
        let (id, deck, seen) = probe();

        ask(&client, Command::Host(HostCommand::Register { id, deck }))
            .expect("the session takes the deck");

        assert!(matches!(next(&seen), Seen::Held(_)));
        assert!(
            matches!(next(&seen), Seen::Drained(thread) if thread.as_deref() == Some(consts::DECK_SESSION)),
            "the session thread drains a deck it takes before ticking it"
        );
        shut_down(&client, worker);
    }

    #[kithara::test]
    fn the_session_thread_ticks_a_held_deck_until_it_is_released() {
        let (client, worker, _view) = deck_session();
        let (id, deck, seen) = probe();

        ask(&client, Command::Host(HostCommand::Register { id, deck }))
            .expect("the session takes the deck");
        let mut ticked = 0;
        while ticked < 2 {
            if let Seen::Ticked(thread) = next(&seen) {
                assert_eq!(thread.as_deref(), Some(consts::DECK_SESSION));
                ticked += 1;
            }
        }
        // The session hands the deck back by value: nothing it does later
        // reaches it, so the record ends with the hand-back.
        ask(&client, Command::Host(HostCommand::Release(id)))
            .expect("the session closes the deck scope");

        let seen = so_far(&seen);
        let at = seen
            .iter()
            .position(|seen| matches!(seen, Seen::Released))
            .expect("the deck comes back released");
        assert_eq!(ticks(&seen[at..]), 0, "a released deck is no longer ticked");
        shut_down(&client, worker);
    }

    #[kithara::test]
    fn a_deck_the_session_lets_go_is_released_before_it_is_handed_back() {
        let (client, worker, _view) = deck_session();
        let (id, deck, seen) = probe();
        ask(&client, Command::Host(HostCommand::Register { id, deck }))
            .expect("the session takes the deck");

        ask(&client, Command::Host(HostCommand::Release(id)))
            .expect("the session closes the deck scope");

        assert!(
            so_far(&seen)
                .iter()
                .any(|seen| matches!(seen, Seen::Released)),
            "a deck comes back released, so nothing waits on what it had queued"
        );
        shut_down(&client, worker);
    }

    /// The session lets go of its decks on its own thread as it shuts down:
    /// each is released, then dropped, before the shutdown answers.
    #[kithara::test]
    fn shutdown_lets_go_of_every_deck_before_it_answers() {
        let (client, worker, _view) = deck_session();
        let (id, deck, seen) = probe();
        ask(&client, Command::Host(HostCommand::Register { id, deck }))
            .expect("the session takes the deck");

        shut_down(&client, worker);

        let after = so_far(&seen);
        let released = after.iter().position(|seen| matches!(seen, Seen::Released));
        let dropped = after.iter().position(|seen| matches!(seen, Seen::Dropped));
        assert!(
            matches!((released, dropped), (Some(released), Some(dropped)) if released < dropped),
            "a deck is released, then dropped"
        );
    }

    #[kithara::test]
    fn a_woken_deck_drains_on_the_session_thread() {
        let (client, worker, _view) = deck_session();
        let (id, deck, seen) = probe();
        ask(&client, Command::Host(HostCommand::Register { id, deck }))
            .expect("the session takes the deck");
        let Seen::Held(waker) = next(&seen) else {
            panic!("the session holds the deck before anything else");
        };
        assert!(
            matches!(next(&seen), Seen::Drained(_)),
            "drained as it is held"
        );

        waker.wake_by_ref();

        loop {
            match next(&seen) {
                Seen::Drained(thread) => {
                    assert_eq!(thread.as_deref(), Some(consts::DECK_SESSION));
                    break;
                }
                Seen::Ticked(_) => {}
                _ => panic!("a woken deck is drained while it stays held"),
            }
        }
        shut_down(&client, worker);
    }

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
        let (client, worker, root_view) = deck_session();
        let (id, deck, _seen) = probe();
        ask(&client, Command::Host(HostCommand::Register { id, deck })).expect("fixture deck");
        let _ = render(&client, 0);
        let mut clock_samples = 512;
        runtime
            .block_on(wait_until(
                Duration::from_secs(2),
                "native Host delivers graph change",
                || {
                    let _ = render(&client, clock_samples);
                    clock_samples += 512;
                    root_view.grid().state() == BeatGridState::Live
                },
            ))
            .expect("the transport's own tempo reaches the read-only Host view without a command");

        assert!(matches!(
            ask(
                &client,
                Command::Host(HostCommand::Configure(
                    HostSettingsChange::Metronome(MetronomeConfigChange::Enabled(true)),
                    When::Next
                ))
            ),
            Ok(())
        ));
        let mut on_blocks = 0;
        runtime
            .block_on(wait_until(
                Duration::from_secs(3),
                "idle native metronome sounds",
                || {
                    let mut sounded = false;
                    for _ in 0..100 {
                        let pcm = render(&client, clock_samples);
                        clock_samples += 512;
                        on_blocks += 1;
                        sounded |= pcm.iter().any(|sample| sample.abs() > 0.01);
                    }
                    sounded
                },
            ))
            .unwrap_or_else(|error| {
                panic!("metronome on reaches PCM: {error}; blocks={on_blocks}")
            });

        assert!(matches!(
            ask(
                &client,
                Command::Host(HostCommand::Configure(
                    HostSettingsChange::Metronome(MetronomeConfigChange::Enabled(false)),
                    When::Next
                ))
            ),
            Ok(())
        ));
        runtime
            .block_on(wait_until(
                Duration::from_secs(4),
                "idle native metronome stays off",
                || {
                    let mut silent = true;
                    for _ in 0..100 {
                        let pcm = render(&client, clock_samples);
                        clock_samples += 512;
                        silent &= pcm.iter().all(|sample| sample.abs() < 1e-6);
                    }
                    silent
                },
            ))
            .expect("the muted capture spans more than one beat");
        shut_down(&client, worker);
    }
}
