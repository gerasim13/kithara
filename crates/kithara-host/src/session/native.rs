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
    state::{HostRoot, RootView, SessionState, SessionStream},
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
    cmd_rx: mpsc::Receiver<EngineMsg>,
    mut mailbox: HostMailbox<O::Command>,
    mut owner: O,
) {
    let mut posts = OwnerPosts::new();
    let mut deadline = Instant::now() + consts::SESSION_PUMP_INTERVAL;
    loop {
        let Ok(message) = receive_message(&cmd_rx, true, deadline) else {
            break;
        };
        owner.begin_pass();
        let mut next = message;
        let mut shutdown = false;
        loop {
            match next {
                Some(EngineMsg::Posted) => posts.drain(&mut owner, &mut mailbox),
                Some(EngineMsg::Deck(message)) => message.run(&mut owner),
                Some(EngineMsg::Shutdown) => { shutdown = true; break; }
                None => {}
            }
            match cmd_rx.try_recv() {
                Ok(message) => next = Some(message),
                Err(_) => break,
            }
        }
        posts.drain(&mut owner, &mut mailbox);
        posts.pass(&mut owner);
        if shutdown { break; }
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
        let state = SessionState::new(
            root,
            view,
            output_block_frames,
            None,
            output,
            settings,
            channel_config,
            start,
        );
        engine_thread(cmd_rx, mailbox, layer(HostCore::new(state, inbox)));
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
    use kithara_command::When;
    use kithara_effects::LimiterConfig;
    use kithara_events::EventBus;
    use kithara_platform::time::Duration;
    use kithara_play::{DeckMixerConfig, DeckRegistration};
    use kithara_test_utils::{
        bufpool::{TestPools, pools},
        kithara, wait_until,
    };
    use kithara_warp::{BeatGridId, BeatGridState};

    use super::*;
    use crate::{
        HostSettingsChange, MetronomeConfigChange,
        session::{
            decks::SessionDecks,
            protocol::ask,
            tests::{
                deck_probe::{Seen, next, probe, so_far, ticks},
                graph::empty_root,
                ring::{MasterRing, RingBackend, RingBackendConfig, RingLayout},
            },
        },
    };

    /// A session with no graph that holds probe decks.
    fn deck_session() -> (Arc<SessionClient<TestPools>>, SessionDecks) {
        let sample_rate = NonZeroU32::new(48_000).expect("test sample rate");
        let (root, root_view) = empty_root(sample_rate);
        let client = spawn_session_client::<(), TestPools>(
            consts::DECK_SESSION,
            root,
            root_view,
            None,
            SessionOutput::new(LimiterConfig::default()),
            Live::new(HostSettings::builder().sample_rate(sample_rate).build())
                .expect("the fixture settings are valid"),
            |_, _| Err("a probe deck opens no stream".to_owned()),
        );
        let decks = SessionDecks::new(client.clone());
        (client, decks)
    }

    fn shut_down(client: &SessionClient<TestPools>) {
        assert!(ask(client, HostCmd::Shutdown).is_ok());
    }

    /// Commands posted before the deck was held woke no one, so the session
    /// thread runs them as soon as it holds the deck.
    #[kithara::test]
    fn the_session_thread_drains_a_deck_as_it_takes_it() {
        let (client, decks) = deck_session();
        let (id, deck, seen) = probe();

        decks.hold(id, deck).expect("the session takes the deck");

        assert!(matches!(next(&seen), Seen::Held(_)));
        assert!(
            matches!(next(&seen), Seen::Drained(thread) if thread.as_deref() == Some(consts::DECK_SESSION)),
            "the session thread drains a deck it takes before ticking it"
        );
        shut_down(&client);
    }

    #[kithara::test]
    fn the_session_thread_ticks_a_held_deck_until_it_is_released() {
        let (client, decks) = deck_session();
        let (id, deck, seen) = probe();

        decks.hold(id, deck).expect("the session takes the deck");
        let mut ticked = 0;
        while ticked < 2 {
            if let Seen::Ticked(thread) = next(&seen) {
                assert_eq!(thread.as_deref(), Some(consts::DECK_SESSION));
                ticked += 1;
            }
        }
        // The session hands the deck back by value: nothing it does later
        // reaches it, so the record ends with the hand-back.
        let released = decks.release(id).expect("the session hands the deck back");

        let seen = so_far(&seen);
        let at = seen
            .iter()
            .position(|seen| matches!(seen, Seen::Released))
            .expect("the deck comes back released");
        assert_eq!(ticks(&seen[at..]), 0, "a released deck is no longer ticked");
        drop(released);
        shut_down(&client);
    }

    #[kithara::test]
    fn a_deck_the_session_lets_go_is_released_before_it_is_handed_back() {
        let (client, decks) = deck_session();
        let (id, deck, seen) = probe();
        decks.hold(id, deck).expect("the session takes the deck");

        let released = decks.release(id).expect("the session hands the deck back");

        assert!(
            so_far(&seen)
                .iter()
                .any(|seen| matches!(seen, Seen::Released)),
            "a deck comes back released, so nothing waits on what it had queued"
        );
        drop(released);
        shut_down(&client);
    }

    /// The session lets go of its decks on its own thread as it shuts down:
    /// each is released, then dropped, before the shutdown answers.
    #[kithara::test]
    fn shutdown_lets_go_of_every_deck_before_it_answers() {
        let (client, decks) = deck_session();
        let (id, deck, seen) = probe();
        decks.hold(id, deck).expect("the session takes the deck");

        shut_down(&client);

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
        let (client, decks) = deck_session();
        let (id, deck, seen) = probe();
        decks.hold(id, deck).expect("the session takes the deck");
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
        shut_down(&client);
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
        let sample_rate = NonZeroU32::new(48_000).expect("test sample rate");
        let (root, root_view) = empty_root(sample_rate);
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

        client
            .attach(DeckRegistration::new(
                BeatGridId::allocate().expect("fixture player grid id"),
                EventBus::default(),
                pools(),
                DeckMixerConfig::default(),
            ))
            .expect("the session starts the fixture deck");
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
            ask(
                &*client,
                HostCmd::Configure {
                    change: HostSettingsChange::Metronome(MetronomeConfigChange::Enabled(true)),
                    at: When::Next,
                }
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
            ask(
                &*client,
                HostCmd::Configure {
                    change: HostSettingsChange::Metronome(MetronomeConfigChange::Enabled(false)),
                    at: When::Next,
                }
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
        assert!(ask(&*client, HostCmd::Shutdown).is_ok());
    }
}
