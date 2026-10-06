#![cfg(not(target_arch = "wasm32"))]

use std::num::{NonZeroU32, NonZeroUsize};

use kithara::{
    host::{HostConfig, HostSettings},
    play::{
        BufferGeometryError, PlayError, PlayWorker, PlayWorkerConfig, PlayerConfig, PlayerImpl,
        SessionError, SessionEvent,
    },
    warp::WarpConfig,
};
use kithara_integration_tests::{offline::OfflineHostHarness, smoothing::consts};
use kithara_test_utils::bufpool::{Pools, TestPools, pools};

/// An offline Host at the suite's rate and block, drawing on `region`.
async fn offline_host(region: &Pools) -> OfflineHostHarness<TestPools> {
    let config = HostConfig::offline(region.clone())
        .settings(HostSettings::builder().sample_rate(sample_rate()).build())
        .max_block_frames(
            u32::try_from(consts::BLOCK_FRAMES)
                .ok()
                .and_then(NonZeroU32::new)
                .expect("block size"),
        )
        .build();
    OfflineHostHarness::new(config).await.expect("offline host")
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
            host.output_sample_rate()
                .expect("host sample rate")
                .measured
                .is_none(),
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
        assert!(
            host.output_sample_rate()
                .expect("sample rate")
                .measured
                .is_none(),
            "inserting an idle deck must not start the output stream"
        );
        deck.set_eq_gain(0, -6.0).expect("configure idle EQ");
        assert_eq!(deck.eq_gain(0), Some(-6.0));
        deck.play();
        assert!(
            host.output_sample_rate()
                .expect("sample rate")
                .measured
                .is_some()
        );
        assert_eq!(deck.eq_gain(0), Some(-6.0));
        deck.pause();
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
