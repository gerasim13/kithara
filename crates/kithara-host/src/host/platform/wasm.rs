use std::{
    cell::RefCell,
    mem,
    num::NonZeroU32,
    rc::{Rc, Weak},
    task::{Wake, Waker},
};

use kithara_bufpool::HasPool;
use kithara_command::Live;
use kithara_platform::{
    sync::{Arc, Mutex},
    time,
    tokio::task,
};
use kithara_play::{PlayError, player::PlayerControlSource};
use kithara_warp::BeatGridId;
use send_wrapper::SendWrapper;

use super::{
    super::{Host, HostOwned, owner::SessionRuntime},
    decks::{Deck, Decks},
};
use crate::{
    HostSettings, consts,
    rt::SessionOutput,
    session::{HostDispatcher, HostProtocol, HostRoot, RootView, web::WebSessionState},
    wasm::HostRoute,
};
/// The decks a Worker Host holds. The Worker is one thread: the Host borrows
/// them between its calls, its clock between two sleeps.
type WorkerDecks = Rc<RefCell<Decks>>;
type StartedPlatform<S> = (Arc<dyn HostDispatcher<S>>, Platform<S>);

pub(in crate::host) struct Platform<S> {
    remote_routes: Mutex<Vec<Arc<HostRoute<S>>>>,
    remote_decks: Option<WorkerDecks>,
    web_state: Option<WebSessionState<S>>,
}

impl<S> Platform<S> {
    pub(in crate::host) fn close(platform: &mut Self, host_id: BeatGridId) {
        for route in mem::take(&mut *platform.remote_routes.lock()) {
            route.close();
        }
        if let Some(decks) = platform.remote_decks.take() {
            let mut decks = mem::take(&mut *decks.borrow_mut());
            decks.release_all();
            let mut deck_count = 0_usize;
            for deck in decks {
                deck_count += 1;
                mem::forget(deck);
            }
            if deck_count > 0 {
                tracing::error!(
                    ?host_id,
                    deck_count,
                    "remote wasm Host dropped before its players detached; retaining decks"
                );
            }
        }
    }

    fn close_deck(&self, id: BeatGridId) -> Result<(), PlayError> {
        self.worker_decks()?.borrow_mut().close(id)
    }

    #[cfg(feature = "offline")]
    pub(in crate::host) fn offline() -> Result<Self, PlayError> {
        if kithara_platform::thread::is_main_thread() {
            return Err(PlayError::SessionCategoryUnsupported {
                reason: "offline Host must run in a Web Worker".to_owned(),
            });
        }
        Ok(Self::remote(WorkerDecks::default()))
    }

    /// Ticks the decks of an offline session ahead of one rendered block.
    #[cfg(feature = "offline")]
    pub(in crate::host) fn tick_block(&self) {
        if let Some(decks) = &self.remote_decks {
            decks.borrow_mut().tick();
        }
    }

    pub(in crate::host) fn owner(web_state: WebSessionState<S>) -> Self {
        Self {
            web_state: Some(web_state),
            remote_routes: Mutex::default(),
            remote_decks: None,
        }
    }

    pub(in crate::host) fn realtime(
        root: HostRoot,
        view: RootView,
        _output_block_frames: Option<NonZeroU32>,
        output: SessionOutput,
        settings: Live<HostSettings, HostProtocol>,
    ) -> Result<StartedPlatform<S>, PlayError>
    where
        S: HasPool<f32> + Send + Sync + 'static,
    {
        let (dispatcher, web_state) =
            crate::session::web::spawn::<S>(root, view, output, settings)?;
        Ok((dispatcher, Self::owner(web_state)))
    }

    fn release_deck(&self, id: BeatGridId) -> Result<(), PlayError> {
        self.worker_decks()?.borrow_mut().release(id).map(drop)
    }

    fn release_on_session_gone<T>(
        &self,
        id: BeatGridId,
        result: Result<T, PlayError>,
    ) -> Result<T, PlayError> {
        match result {
            Err(error @ PlayError::SessionGone { .. }) => {
                self.release_deck(id)?;
                Err(error)
            }
            result => result,
        }
    }

    fn remote(decks: WorkerDecks) -> Self {
        Self {
            web_state: None,
            remote_routes: Mutex::default(),
            remote_decks: Some(decks),
        }
    }

    fn worker_decks(&self) -> Result<&WorkerDecks, PlayError> {
        self.remote_decks.as_ref().ok_or_else(|| {
            PlayError::Internal("wasm players must be inserted from their owning Worker".into())
        })
    }
}

/// Ticks a realtime Worker Host's decks once per session pump interval, on
/// the Worker that holds them, until the Host lets them go.
fn spawn_clock(decks: Weak<RefCell<Decks>>) {
    task::spawn(async move {
        loop {
            time::sleep(consts::SESSION_PUMP_INTERVAL).await;
            let Some(decks) = decks.upgrade() else {
                break;
            };
            decks.borrow_mut().tick();
        }
    });
}

/// Drains one deck on the Worker that holds it, inside the wake: a caller on
/// that Worker blocks on its answer, so nothing else would run the deck
/// before the caller reads it.
struct WorkerWake {
    id: BeatGridId,
    decks: SendWrapper<Weak<RefCell<Decks>>>,
}

impl WorkerWake {
    fn waker(decks: &WorkerDecks, id: BeatGridId) -> Waker {
        Waker::from(Arc::new(Self {
            id,
            decks: SendWrapper::new(Rc::downgrade(decks)),
        }))
    }
}

impl Wake for WorkerWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    /// Decks the Host already let go of released this one, so a wake that
    /// finds them gone had nothing left to run.
    fn wake_by_ref(self: &Arc<Self>) {
        if let Some(decks) = self.decks.upgrade() {
            decks.borrow_mut().drain(self.id);
        }
    }
}

impl<S> Host<S>
where
    S: HasPool<f32> + Send + Sync + 'static,
{
    /// Attaches and transfers one fully configured player or decorator into
    /// this Host, which starts its deck before returning: the output runs
    /// from now until the Host hands its last deck back. Musical playback
    /// remains stopped.
    ///
    /// # Errors
    /// Returns an error when binding, attachment, or starting the deck fails.
    pub fn insert<P>(&mut self, mut player: P) -> Result<HostOwned<P>, PlayError>
    where
        P: PlayerControlSource<Schema = S>,
    {
        let decks = Rc::clone(self.session.platform().worker_decks()?);
        let (grid_id, control) = self.bind_player(&mut player)?;
        let mut deck: Deck = Box::new(player);
        deck.hold(WorkerWake::waker(&decks, grid_id));
        decks.borrow_mut().hold(grid_id, deck);
        Ok(self.owned::<P>(grid_id, control))
    }

    pub(crate) fn register_remote_route(&self, route: Arc<HostRoute<S>>) {
        self.session.platform().remote_routes.lock().push(route);
    }

    pub(crate) fn remote(
        id: BeatGridId,
        root_view: RootView,
        dispatcher: Arc<dyn HostDispatcher<S>>,
    ) -> Self {
        let decks = WorkerDecks::default();
        spawn_clock(Rc::downgrade(&decks));
        Self {
            id,
            root_view,
            dispatcher,
            owns_session: false,
            session: SessionRuntime::realtime(Platform::remote(decks)),
        }
    }

    pub(crate) fn remote_identity(&self) -> (BeatGridId, RootView) {
        (self.id, self.root_view.clone())
    }

    /// Closes the deck where the Host holds it, stops it and detaches it
    /// from the session, then drops it.
    ///
    /// # Errors
    /// Returns an error when close or canonical detachment fails.
    pub fn remove<P>(&mut self, player: &HostOwned<P>) -> Result<(), PlayError>
    where
        P: PlayerControlSource<Schema = S>,
    {
        self.validate_removal(player)?;
        self.remove_deck(player.id())
    }

    fn remove_deck(&self, id: BeatGridId) -> Result<(), PlayError> {
        let platform = self.session.platform();
        platform.release_on_session_gone(id, platform.close_deck(id))?;
        platform.release_on_session_gone(id, self.dispatcher.detach(id))?;
        platform.release_deck(id)
    }

    pub(crate) fn web_state(&self) -> Option<&WebSessionState<S>> {
        self.session.platform().web_state.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::{Cell, RefCell},
        num::NonZeroU32,
        rc::Rc,
        task::Waker,
    };

    use delegate::delegate;
    use kithara_audio::ConsumerWakeMode;
    use kithara_platform::{sync::Arc, time};
    use kithara_play::{
        Cmd, PlayError, Reply, SessionDispatcher, SessionSampleRate, StreamShape, player::Player,
    };
    use kithara_test_utils::{bufpool::TestPools, kithara};
    use kithara_warp::BeatGridId;

    use super::{Host, Platform, SessionRuntime, WorkerDecks, WorkerWake, spawn_clock};
    use crate::{
        HostSettings, consts,
        host::owner::SessionRoot,
        session::{HostCmd, HostDispatcher, HostReply, HostRoot, protocol::HostDispatchError},
    };

    struct FixtureSession;

    impl<S> SessionDispatcher<S> for FixtureSession {
        fn consumer_wake_mode(&self) -> ConsumerWakeMode {
            ConsumerWakeMode::RealtimeDeferred
        }

        fn exec(&self, _cmd: Cmd) -> Result<Reply, PlayError> {
            Ok(Reply::Ok)
        }

        fn sample_rate(&self) -> SessionSampleRate {
            SessionSampleRate::new(None, consts::DEFAULT_SAMPLE_RATE.get())
        }

        fn stream_shape(&self) -> Option<StreamShape> {
            None
        }
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Outcome {
        Ok,
        SessionGone,
        OtherError,
    }

    /// A deck that closes with a chosen outcome and counts its drains, ticks
    /// and drops.
    struct DeckProbe {
        close: Outcome,
        drains: Rc<Cell<usize>>,
        drops: Rc<RefCell<usize>>,
        ticks: Rc<Cell<usize>>,
    }

    impl Drop for DeckProbe {
        fn drop(&mut self) {
            *self.drops.borrow_mut() += 1;
        }
    }

    impl Player for DeckProbe {
        fn close(&mut self) -> Result<(), PlayError> {
            match self.close {
                Outcome::Ok => Ok(()),
                Outcome::SessionGone => Err(PlayError::SessionGone {
                    reason: "fixture deck close",
                }),
                Outcome::OtherError => Err(PlayError::Internal("fixture deck close failed".into())),
            }
        }

        fn drain(&mut self) {
            self.drains.set(self.drains.get() + 1);
        }

        fn hold(&mut self, _waker: Waker) {}

        fn release(&mut self) {}

        fn tick(&mut self) -> Result<(), PlayError> {
            self.ticks.set(self.ticks.get() + 1);
            Ok(())
        }
    }

    fn deck(close: Outcome, drops: &Rc<RefCell<usize>>) -> Box<DeckProbe> {
        Box::new(DeckProbe {
            close,
            drains: Rc::default(),
            drops: Rc::clone(drops),
            ticks: Rc::default(),
        })
    }

    struct Dispatcher {
        session: FixtureSession,
        detach: Outcome,
        root: RefCell<HostRoot>,
    }

    impl SessionDispatcher<TestPools> for Dispatcher {
        delegate! {
            to &self.session {
                #[through(SessionDispatcher::<TestPools>)]
                fn exec(&self, cmd: Cmd) -> Result<Reply, PlayError>;
                #[through(SessionDispatcher::<TestPools>)]
                fn consumer_wake_mode(&self) -> ConsumerWakeMode;
                #[through(SessionDispatcher::<TestPools>)]
                fn sample_rate(&self) -> SessionSampleRate;
                #[through(SessionDispatcher::<TestPools>)]
                fn stream_shape(&self) -> Option<StreamShape>;
            }
        }
    }

    impl HostDispatcher<TestPools> for Dispatcher {
        fn exec_host(
            &self,
            cmd: HostCmd<TestPools>,
        ) -> Result<HostReply, HostDispatchError<TestPools>> {
            let HostCmd::Detach { grid_id } = cmd else {
                panic!("unexpected fixture Host command")
            };
            match self.detach {
                Outcome::SessionGone => Err(HostDispatchError::before_send(
                    PlayError::SessionGone {
                        reason: "fixture detach",
                    },
                    HostCmd::Detach { grid_id },
                )),
                Outcome::OtherError => Ok(HostReply::Err(PlayError::Internal(
                    "fixture detach failed".into(),
                ))),
                Outcome::Ok => Ok(self
                    .root
                    .borrow_mut()
                    .detach(grid_id)
                    .map_or_else(|error| HostReply::Err(error.into()), |()| HostReply::Ok)),
            }
        }
    }

    struct Fixture {
        host: Host<TestPools>,
        deck: BeatGridId,
        drops: Rc<RefCell<usize>>,
        ticks: Rc<Cell<usize>>,
    }

    fn fixture(close: Outcome, detach: Outcome) -> Fixture {
        let sample_rate = NonZeroU32::new(44_100).expect("fixture sample rate");
        let SessionRoot {
            id: host_id,
            mut root,
            view: root_view,
        } = Host::<TestPools>::session_root(
            HostSettings::builder().sample_rate(sample_rate).build(),
        )
        .expect("fixture Host session");
        let deck_id = BeatGridId::allocate().expect("fixture deck grid id");
        root.attach(deck_id).expect("fixture deck attachment");

        let dispatcher: Arc<dyn HostDispatcher<TestPools>> = Arc::new(Dispatcher {
            detach,
            session: FixtureSession,
            root: RefCell::new(root),
        });
        let drops = Rc::default();
        let probe = deck(close, &drops);
        let ticks = Rc::clone(&probe.ticks);
        let decks = WorkerDecks::default();
        decks.borrow_mut().hold(deck_id, probe);
        let host = Host {
            root_view,
            dispatcher,
            id: host_id,
            owns_session: false,
            session: SessionRuntime::realtime(Platform::remote(decks)),
        };
        Fixture {
            host,
            deck: deck_id,
            drops,
            ticks,
        }
    }

    #[kithara::test(wasm, flash(false))]
    fn successful_remove_releases_the_deck() {
        let Fixture {
            host, deck, drops, ..
        } = fixture(Outcome::Ok, Outcome::Ok);

        host.remove_deck(deck).expect("remove deck");

        assert_eq!(*drops.borrow(), 1);
    }

    #[kithara::test(wasm, flash(false))]
    #[case::closing(Outcome::SessionGone, Outcome::Ok)]
    #[case::detaching(Outcome::Ok, Outcome::SessionGone)]
    fn session_gone_releases_the_deck(#[case] close: Outcome, #[case] detach: Outcome) {
        let Fixture {
            host, deck, drops, ..
        } = fixture(close, detach);

        assert!(matches!(
            host.remove_deck(deck),
            Err(PlayError::SessionGone { .. })
        ));
        assert_eq!(*drops.borrow(), 1);
    }

    #[kithara::test(wasm, flash(false))]
    fn other_errors_retain_the_deck() {
        for (close, detach) in [
            (Outcome::OtherError, Outcome::Ok),
            (Outcome::Ok, Outcome::OtherError),
        ] {
            let Fixture {
                host,
                deck,
                drops,
                ticks,
            } = fixture(close, detach);

            assert!(matches!(
                host.remove_deck(deck),
                Err(PlayError::Internal(_))
            ));
            assert_eq!(*drops.borrow(), 0);
            host.session
                .platform()
                .worker_decks()
                .expect("a Worker Host holds decks")
                .borrow_mut()
                .tick();
            assert_eq!(ticks.get(), 1, "the Host still holds the deck");
            drop(host);
            assert_eq!(*drops.borrow(), 0);
        }
    }

    #[kithara::test(wasm)]
    fn a_wake_drains_a_worker_deck_before_it_returns() {
        let decks = WorkerDecks::default();
        let probe = deck(Outcome::Ok, &Rc::default());
        let drains = Rc::clone(&probe.drains);
        let id = BeatGridId::allocate().expect("fixture deck grid id");
        let waker = WorkerWake::waker(&decks, id);
        decks.borrow_mut().hold(id, probe);
        assert_eq!(drains.get(), 1, "held, the deck runs what waited for it");

        waker.wake_by_ref();

        assert_eq!(
            drains.get(),
            2,
            "a caller on the Worker reads its answer right after it posts"
        );
    }

    #[kithara::test(wasm)]
    async fn the_worker_clock_ticks_a_held_deck_until_it_is_released() {
        let decks = WorkerDecks::default();
        spawn_clock(Rc::downgrade(&decks));
        let drops = Rc::default();
        let probe = deck(Outcome::Ok, &drops);
        let ticks = Rc::clone(&probe.ticks);
        let id = BeatGridId::allocate().expect("fixture deck grid id");

        decks.borrow_mut().hold(id, probe);
        while ticks.get() < 2 {
            time::sleep(consts::SESSION_PUMP_INTERVAL).await;
        }
        let released = decks
            .borrow_mut()
            .release(id)
            .expect("the Host takes the deck back");
        let at_release = ticks.get();
        time::sleep(consts::SESSION_PUMP_INTERVAL * 3).await;

        assert_eq!(
            ticks.get(),
            at_release,
            "a released deck is no longer ticked"
        );
        drop(released);
    }
}
