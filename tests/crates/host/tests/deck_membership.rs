#![cfg(not(target_arch = "wasm32"))]

use std::{
    num::{NonZeroU32, NonZeroUsize},
    task::Waker,
};

use delegate::delegate;
use kithara::{
    host::{HostConfig, HostSettings},
    platform::{
        sync::{Arc, Mutex},
        thread,
    },
    play::{
        AllocatedSlot, BufferGeometryError, DeckRegistration, PlayError, PlayWorker,
        PlayWorkerConfig, PlayerConfig, PlayerImpl, SessionBinding, SessionError, SessionEvent,
        player::{Player, PlayerControlSource},
    },
    warp::WarpConfig,
    worker::DispatcherConfig,
};
use kithara_integration_tests::{offline::OfflineHostHarness, smoothing::consts};
use kithara_test_utils::bufpool::{Pools, TestPools, pools};

/// An offline Host at the suite's rate and block, drawing on `region`.
async fn offline_host(region: &Pools) -> OfflineHostHarness<TestPools> {
    let config = HostConfig::offline(region.clone())
        .settings(HostSettings::builder().sample_rate(sample_rate()).build())
        .max_block_frames(block_frames())
        .build();
    OfflineHostHarness::new(config).await.expect("offline host")
}

fn block_frames() -> NonZeroU32 {
    u32::try_from(consts::BLOCK_FRAMES)
        .ok()
        .and_then(NonZeroU32::new)
        .expect("block size")
}

fn sample_rate() -> NonZeroU32 {
    NonZeroU32::new(consts::SAMPLE_RATE).expect("sample rate")
}

#[kithara::test(tokio)]
async fn failed_deck_preparation_releases_host_membership() {
    let region = pools();
    let sample_rate = sample_rate();
    let host = offline_host(&region).await;
    let worker = PlayWorker::new(PlayWorkerConfig::builder(region).build());
    let invalid = PlayerImpl::new(
        PlayerConfig::builder()
            .sample_rate(sample_rate)
            .worker(worker.clone())
            .warp(
                WarpConfig::builder()
                    .render_quantum_frames(NonZeroUsize::new(32).expect("quantum"))
                    .build(),
            )
            .response_budget_frames(NonZeroUsize::new(1).expect("budget"))
            .build(),
    );
    assert!(matches!(
        host.insert(invalid).await,
        Err(PlayError::Session(SessionError::BufferGeometry(
            BufferGeometryError::BudgetExceeded { .. }
        )))
    ));
    host.with(|host| {
        assert!(host.is_empty());
        assert!(
            host.output_sample_rate().measured.is_none(),
            "failed preparation must close an otherwise idle stream"
        );
    })
    .await;
    let valid = PlayerImpl::new(
        PlayerConfig::builder()
            .sample_rate(sample_rate)
            .worker(worker)
            .build(),
    );
    let deck = host
        .insert(valid)
        .await
        .expect("host can prepare the next deck");
    host.with(move |host| {
        deck.set_eq_gain(0, -6.0).expect("configure idle EQ");
        assert_eq!(deck.eq_gain(0), Some(-6.0));
        deck.play();
        assert!(host.output_sample_rate().measured.is_some());
        assert_eq!(deck.eq_gain(0), Some(-6.0));
        deck.pause();
    })
    .await;
    host.close().await;
}

/// The Host starts a deck as it takes it and stops it as it hands it back, so
/// the deck's output runs for its whole membership with nothing played.
#[kithara::test(tokio)]
async fn a_deck_runs_from_its_insert_to_its_remove() {
    let region = pools();
    let host = offline_host(&region).await;
    let worker = PlayWorker::new(PlayWorkerConfig::builder(region).build());
    let deck = host
        .insert(PlayerImpl::new(
            PlayerConfig::builder()
                .sample_rate(sample_rate())
                .worker(worker)
                .build(),
        ))
        .await
        .expect("the Host takes the deck");
    host.with(move |host| {
        assert!(
            host.output_sample_rate().measured.is_some(),
            "the Host starts a deck as it takes it"
        );
        host.remove(&deck).expect("the Host hands the deck back");
        assert!(host.is_empty());
        assert!(
            host.output_sample_rate().measured.is_none(),
            "the last deck the Host hands back takes the output with it"
        );
    })
    .await;
    host.close().await;
}

#[kithara::test(tokio)]
async fn a_route_change_reaches_every_deck_the_host_holds() {
    let region = pools();
    let host = offline_host(&region).await;
    let worker = PlayWorker::new(PlayWorkerConfig::builder(region).build());
    let mut decks = Vec::new();
    for _ in 0..2 {
        let deck = host
            .insert(PlayerImpl::new(
                PlayerConfig::builder()
                    .sample_rate(sample_rate())
                    .worker(worker.clone())
                    .build(),
            ))
            .await
            .expect("the Host takes the deck");
        decks.push((deck.bus().subscribe::<SessionEvent>(), deck));
    }

    host.invalidate_audio_route("oldDeviceUnavailable")
        .await
        .expect("the Host restarts its route");

    for (heard, _deck) in &mut decks {
        assert!(
            matches!(
                heard.try_recv().map(|envelope| envelope.event),
                Ok(SessionEvent::RouteChanged { .. })
            ),
            "every deck the Host holds hears the route change"
        );
    }
    host.close().await;
}

/// A drain or a tick of a deck, with the thread it ran on.
#[derive(Clone, Debug, PartialEq)]
enum Call {
    Drain(Option<String>),
    Tick(Option<String>),
}

/// A player that notes each drain and tick of the player it wraps, in order.
struct ThreadProbe<P> {
    inner: P,
    seen: Arc<Mutex<Vec<Call>>>,
}

impl<P> ThreadProbe<P> {
    fn note(&self, call: fn(Option<String>) -> Call) {
        self.seen
            .lock()
            .push(call(thread::current().name().map(str::to_owned)));
    }
}

impl<P: Player> Player for ThreadProbe<P> {
    fn drain(&mut self) {
        self.note(Call::Drain);
        self.inner.drain();
    }

    fn tick(&mut self) -> Result<(), PlayError> {
        self.note(Call::Tick);
        self.inner.tick()
    }

    delegate! {
        to self.inner {
            fn close(&mut self) -> Result<(), PlayError>;
            fn hold(&mut self, waker: Waker);
            fn release(&mut self);
        }
    }
}

impl<P: PlayerControlSource> PlayerControlSource for ThreadProbe<P> {
    type Control = P::Control;
    type Schema = P::Schema;

    fn close_control(control: &Self::Control) -> Result<(), PlayError> {
        P::close_control(control)
    }

    delegate! {
        to self.inner {
            fn attach_session(
                &mut self,
                binding: SessionBinding,
            ) -> Result<DeckRegistration<Self::Schema>, PlayError>;
            fn control(&self) -> Self::Control;
            fn seat(&mut self, slot: AllocatedSlot);
        }
    }
}

/// The Host's session thread holds its decks: it runs a deck's commands as it
/// takes the deck and ticks the deck once ahead of each block it renders.
#[kithara::test(tokio)]
async fn the_session_thread_drains_and_ticks_the_decks_it_holds() {
    const SESSION: &str = "deck-session";
    const BLOCKS: usize = 3;
    let region = pools();
    let config = HostConfig::offline(region.clone())
        .settings(HostSettings::builder().sample_rate(sample_rate()).build())
        .max_block_frames(block_frames())
        .dispatcher(
            DispatcherConfig::builder()
                .name(SESSION)
                .capacity(NonZeroUsize::MIN)
                .build(),
        )
        .build();
    let host = OfflineHostHarness::new(config).await.expect("offline host");
    let seen = Arc::default();
    host.insert(ThreadProbe {
        inner: PlayerImpl::new(
            PlayerConfig::builder()
                .sample_rate(sample_rate())
                .worker(PlayWorker::new(PlayWorkerConfig::builder(region).build()))
                .build(),
        ),
        seen: Arc::clone(&seen),
    })
    .await
    .expect("the Host takes the deck");
    host.render(consts::BLOCK_FRAMES * BLOCKS).await;

    let session = || Some(SESSION.to_owned());
    let mut expected = vec![Call::Drain(session())];
    expected.extend(vec![Call::Tick(session()); BLOCKS]);
    assert_eq!(
        *seen.lock(),
        expected,
        "a drain as the session thread takes the deck, then one tick ahead of each block"
    );
    host.close().await;
}
