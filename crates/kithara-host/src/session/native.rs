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
use tracing::{debug, warn};

use super::{
    decks::{DeckInbox, DeckMsg, Decks},
    dispatch::{run_host_cmd, tick_session},
    protocol::{HostCmd, HostDispatchError, HostDispatcher, Reply, answer},
    queue::HostProtocol,
    state::{HostRoot, RootView, SessionState},
};
use crate::{HostSettings, consts, error::PlayError, rt::SessionOutput};

/// What the native session thread takes: the Host's commands and the
/// messages for the decks it holds.
enum EngineMsg<S> {
    Host(HostCmd<S>),
    Deck(DeckMsg),
}

pub(crate) struct SessionClient<S> {
    cmd_tx: Mutex<mpsc::Sender<EngineMsg<S>>>,
}

impl<S: Send + Sync + 'static> HostDispatcher<S> for SessionClient<S> {
    fn consumer_wake_mode(&self) -> ConsumerWakeMode {
        ConsumerWakeMode::RealtimeDeferred
    }

    fn dispatch(&self, cmd: HostCmd<S>) -> Result<(), HostDispatchError> {
        self.cmd_tx.lock().send(EngineMsg::Host(cmd)).map_err(|_| {
            HostDispatchError::NotTaken(PlayError::SessionGone {
                reason: "session thread stopped accepting commands",
            })
        })
    }
}

impl<S: Send + Sync + 'static> DeckInbox for SessionClient<S> {
    fn post(&self, message: DeckMsg) -> Result<(), PlayError> {
        self.cmd_tx
            .lock()
            .send(EngineMsg::Deck(message))
            .map_err(|_| PlayError::SessionGone {
                reason: "session thread stopped taking decks",
            })
    }
}

/// Stops the stream with the session state, then drops the decks it
/// rendered, each released first: only the session thread lets go of them.
fn stop<T, S>(state: SessionState<T, S>, mut decks: Decks) {
    decks.release_all();
    drop(state);
    drop(decks);
}

/// Disconnects queued callers and stops the session with its decks before
/// it replies.
fn complete_shutdown<T, S>(
    cmd_rx: mpsc::Receiver<EngineMsg<S>>,
    state: SessionState<T, S>,
    decks: Decks,
    reply: &Reply<()>,
) {
    drop(cmd_rx);
    stop(state, decks);
    answer(reply, ());
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

/// Whether the session pumps on its interval: while it runs a graph or holds
/// a deck.
fn pumping<T, S>(state: &SessionState<T, S>, decks: &Decks) -> bool {
    state.ctx.is_some() || !decks.is_empty()
}

/// One pump once it is due: the decks tick first, so the changes they post
/// reach the graph in the same pump.
fn service_due_tick<T, S>(
    state: &mut SessionState<T, S>,
    decks: &mut Decks,
    deadline: &mut Instant,
) {
    if pumping(state, decks) && Instant::now() >= *deadline {
        decks.tick();
        if state.ctx.is_some()
            && let Err(error) = tick_session(state)
        {
            warn!(?error, "native session tick failed");
        }
        *deadline = Instant::now() + consts::SESSION_PUMP_INTERVAL;
    }
}

fn engine_thread<T, S>(
    cmd_rx: mpsc::Receiver<EngineMsg<S>>,
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
    let mut decks = Decks::default();
    debug!("[KITHARA-ROUTE] native session worker started");
    let mut deadline = Instant::now() + consts::SESSION_PUMP_INTERVAL;
    loop {
        let Ok(message) = receive_message(&cmd_rx, pumping(&state, &decks), deadline) else {
            break;
        };
        match message {
            Some(EngineMsg::Host(HostCmd::Shutdown(reply))) => {
                complete_shutdown(cmd_rx, state, decks, &reply);
                debug!("[KITHARA-ROUTE] native session worker stopped");
                return;
            }
            Some(EngineMsg::Host(cmd)) => run_host_cmd(&mut state, cmd),
            Some(EngineMsg::Deck(message)) => decks.run(message),
            None => {}
        }
        service_due_tick(&mut state, &mut decks, &mut deadline);
    }
    stop(state, decks);
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
    let (cmd_tx, cmd_rx) = mpsc::channel::<EngineMsg<S>>();
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
) -> Arc<SessionClient<S>> {
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
    use kithara_platform::{thread::sleep, time::Duration};
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
        let released = decks.release(id).expect("the session hands the deck back");
        drop(so_far(&seen));
        sleep(consts::SESSION_PUMP_INTERVAL * 3);

        assert_eq!(
            ticks(&so_far(&seen)),
            0,
            "a released deck is no longer ticked"
        );
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
            ask(&*client, |reply| HostCmd::Configure {
                change: HostSettingsChange::Metronome(MetronomeConfigChange::Enabled(true)),
                at: When::Next,
                reply,
            }),
            Ok(Ok(()))
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
            ask(&*client, |reply| HostCmd::Configure {
                change: HostSettingsChange::Metronome(MetronomeConfigChange::Enabled(false)),
                at: When::Next,
                reply,
            }),
            Ok(Ok(()))
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
