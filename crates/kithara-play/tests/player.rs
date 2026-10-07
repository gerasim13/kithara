use std::num::{NonZeroU32, NonZeroUsize};

use kithara_audio::SeekOutcome;
use kithara_decode::GaplessMode;
use kithara_effects::eq::generate_log_spaced_bands;
use kithara_events::{Envelope, EventBus, TrackId, TryRecvError};
use kithara_platform::time::Duration;
use kithara_play::{
    PlayError, PlayWorker, PlayWorkerConfig, PlayerConfig, PlayerEvent, PlayerImpl, PlayerStatus,
    SelectTransition, mock, player::PlayerControlSource,
};
#[cfg(all(test, target_os = "android"))]
use kithara_test_dylib as _;
use kithara_test_utils::{
    bufpool::{TestPools, pools},
    kithara,
};
use kithara_warp::{BeatGridId, WarpConfig};

fn worker() -> PlayWorker<TestPools> {
    PlayWorker::new(PlayWorkerConfig::builder(pools()).build())
}

fn player() -> PlayerImpl<TestPools> {
    PlayerImpl::new(
        PlayerConfig::builder()
            .sample_rate(mock::SAMPLE_RATE)
            .worker(worker())
            .session(mock::session())
            .build(),
    )
}

#[derive(Clone, Copy)]
enum PlayerBasicScenario {
    EngineAccessor,
    NothingIsCurrent,
    StartsPaused,
}

#[kithara::test]
#[case(PlayerBasicScenario::StartsPaused)]
#[case(PlayerBasicScenario::NothingIsCurrent)]
#[case(PlayerBasicScenario::EngineAccessor)]
fn player_basic_behaviors(#[case] scenario: PlayerBasicScenario) {
    let player = player();
    match scenario {
        PlayerBasicScenario::StartsPaused => {
            assert!((player.rate() - 0.0).abs() < f32::EPSILON);
            assert_eq!(player.status(), PlayerStatus::Unknown);
        }
        PlayerBasicScenario::NothingIsCurrent => {
            assert_eq!(player.current_item(), None);
        }
        PlayerBasicScenario::EngineAccessor => {
            assert!(player.engine().slot().is_none());
        }
    }
}

#[kithara::test]
fn player_pause_without_active_slot_keeps_rate_zero() {
    let player = player();
    player.pause();
    assert!((player.rate() - 0.0).abs() < f32::EPSILON);
}

#[kithara::test]
fn player_volume_clamps() {
    let player = player();
    player.set_volume(2.0);
    assert!((player.volume() - 1.0).abs() < f32::EPSILON);
    player.set_volume(-1.0);
    assert!((player.volume() - 0.0).abs() < f32::EPSILON);
}

#[kithara::test]
fn player_muted() {
    let player = player();
    assert!(!player.is_muted());
    player.set_muted(true);
    assert!(player.is_muted());
}

#[kithara::test]
fn player_crossfade_duration() {
    let player = player();
    assert!((player.crossfade_duration() - 1.0).abs() < f32::EPSILON);
    player.set_crossfade_duration(3.0);
    assert!((player.crossfade_duration() - 3.0).abs() < f32::EPSILON);
}

#[kithara::test]
fn player_events_subscribe() {
    let player = player();
    let mut rx = player.subscribe::<PlayerEvent>();
    player.set_volume(0.5);
    let event = rx.try_recv();
    assert!(event.is_ok());
}

#[kithara::test]
fn player_config_sets_capacity_for_a_new_event_bus() {
    let player = PlayerImpl::new(
        PlayerConfig::builder()
            .sample_rate(mock::SAMPLE_RATE)
            .worker(worker())
            .session(mock::session())
            .event_bus_capacity(NonZeroUsize::new(2).expect("two is not zero"))
            .build(),
    );
    let mut rx = player.subscribe::<PlayerEvent>();

    player.set_volume(0.1);
    player.set_volume(0.2);
    player.set_volume(0.3);

    assert!(matches!(rx.try_recv(), Err(TryRecvError::Lagged(1))));
}

#[kithara::test]
fn injected_event_bus_keeps_its_identity_and_capacity() {
    let bus = EventBus::new(1);
    let bus_id = bus.id();
    let player = PlayerImpl::new(
        PlayerConfig::builder()
            .sample_rate(mock::SAMPLE_RATE)
            .worker(worker())
            .session(mock::session())
            .event_bus_capacity(NonZeroUsize::new(8).expect("eight is not zero"))
            .bus(bus)
            .build(),
    );
    let mut rx = player.subscribe::<PlayerEvent>();

    assert_eq!(player.control().bus().id(), bus_id);
    player.set_volume(0.1);
    player.set_volume(0.2);
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Lagged(1))));
}

#[kithara::test]
fn player_config_custom() {
    let config = PlayerConfig::builder()
        .sample_rate(mock::SAMPLE_RATE)
        .worker(worker())
        .session(mock::session())
        .crossfade_duration(2.0)
        .default_rate(0.5)
        .gapless_mode(GaplessMode::MediaOnly)
        .eq_layout(generate_log_spaced_bands(5))
        .warp(WarpConfig::builder().build())
        .build();
    let player = PlayerImpl::new(config);
    assert!((player.crossfade_duration() - 2.0).abs() < f32::EPSILON);
}

/// `PlayerConfig::sample_rate` is the single place the value lives before
/// the `EngineConfig` it configures exists. This pins that the value the
/// player reports back out is the exact one the engine runs with, so the
/// rate the owning Host hands down cannot land somewhere the engine never
/// reads.
#[kithara::test]
fn a_configured_sample_rate_reaches_the_engine_it_prepares() {
    let sample_rate = NonZeroU32::new(48_000).expect("invariant: sample rate is non-zero");
    let player = PlayerImpl::new(
        PlayerConfig::builder()
            .worker(worker())
            .session(mock::session())
            .sample_rate(sample_rate)
            .build(),
    );

    assert_eq!(player.sample_rate(), sample_rate.get());
}

#[kithara::test]
fn eq_band_count_tracks_a_replacement_layout_before_start() {
    let player = PlayerImpl::new(
        PlayerConfig::builder()
            .sample_rate(mock::SAMPLE_RATE)
            .worker(worker())
            .session(mock::session())
            .eq_layout(generate_log_spaced_bands(3))
            .build(),
    );
    assert_eq!(player.eq_band_count(), 3);

    player.set_eq_layout(generate_log_spaced_bands(4)).unwrap();
    assert_eq!(player.eq_band_count(), 4);
}

#[kithara::test]
fn player_config_builder() {
    let config = PlayerConfig::builder()
        .sample_rate(mock::SAMPLE_RATE)
        .worker(worker())
        .session(mock::session())
        .default_rate(0.5)
        .crossfade_duration(2.5)
        .eq_layout(generate_log_spaced_bands(5))
        .build();
    assert!((config.default_rate.load() - 0.5).abs() < f32::EPSILON);
    assert!((config.crossfade_duration.load() - 2.5).abs() < f32::EPSILON);
    assert_eq!(config.eq_layout.len(), 5);
}

#[kithara::test(tokio)]
async fn synchronous_player_events_remain_in_order() {
    let player = player();
    let mut rx = player.subscribe::<PlayerEvent>();

    player.set_volume(0.5);
    player.set_muted(true);
    player.set_rate(2.0);

    let e1 = rx.try_recv();
    let e2 = rx.try_recv();
    assert!(matches!(
        e1,
        Ok(Envelope {
            event: PlayerEvent::VolumeChanged { .. },
            ..
        })
    ));
    assert!(matches!(
        e2,
        Ok(Envelope {
            event: PlayerEvent::MuteChanged { .. },
            ..
        })
    ));
    assert!(
        rx.try_recv().is_err(),
        "rate feedback must wait for the RT processor"
    );
}

#[kithara::test(tokio)]
async fn player_negative_crossfade_duration_clamped() {
    let player = player();
    player.set_crossfade_duration(-5.0);
    assert!((player.crossfade_duration() - 0.0).abs() < f32::EPSILON);
}

#[kithara::test]
fn position_seconds_idle_is_none() {
    let player = player();
    assert!(player.position_seconds().is_none());
    assert!(player.duration_seconds().is_none());
    assert!(!player.is_playing());
    assert!(player.current_abr_handle().is_none());
    assert!(player.armed_next().is_none());
}

#[kithara::test]
fn set_rate_without_rt_does_not_emit_rate_changed() {
    let player = player();
    let mut rx = player.subscribe::<PlayerEvent>();
    player.set_rate(2.0);
    assert!(rx.try_recv().is_err());
}

#[kithara::test]
fn player_keeps_explicit_worker_and_shared_pools() {
    let worker = worker();
    let player = PlayerImpl::new(
        PlayerConfig::builder()
            .sample_rate(mock::SAMPLE_RATE)
            .worker(worker.clone())
            .session(mock::session())
            .build(),
    );
    assert!(std::ptr::eq(player.worker().pools(), worker.pools()));
}

#[kithara::test]
fn a_player_binds_only_the_first_session_and_registers_its_deck_there() {
    let grid_id = BeatGridId::allocate().expect("fixture grid id");
    let mut player = PlayerImpl::new(
        PlayerConfig::builder()
            .grid_id(grid_id)
            .sample_rate(mock::SAMPLE_RATE)
            .worker(worker())
            .build(),
    );

    let deck = PlayerControlSource::attach_session(&mut player, mock::session())
        .expect("the first session binds the player");
    assert_eq!(deck.grid_id, grid_id);
    assert!(
        matches!(
            PlayerControlSource::attach_session(&mut player, mock::session()),
            Err(PlayError::SessionAlreadyBound)
        ),
        "no second session can bind the player"
    );
}

#[kithara::test]
fn host_rejects_a_player_built_for_another_sample_rate() {
    let mut player = PlayerImpl::new(
        PlayerConfig::builder()
            .sample_rate(mock::SAMPLE_RATE)
            .worker(worker())
            .build(),
    );
    let binding = mock::session_at(NonZeroU32::new(48_000).expect("48000 is not zero"));

    assert!(matches!(
        PlayerControlSource::attach_session(&mut player, binding),
        Err(PlayError::SessionSampleRateMismatch {
            player: 44_100,
            session: 48_000,
        })
    ));
}

/// Without a resource the deck can only commit or reselect what it holds:
/// selecting any other item must fail loudly instead of announcing
/// `CurrentItemChanged` while the old audio keeps playing.
#[kithara::test]
fn selecting_an_item_the_deck_lacks_without_a_resource_is_refused() {
    let player = player();
    let item = TrackId::allocate();
    let err = player
        .select_with_crossfade(
            item,
            None,
            SelectTransition {
                playback: kithara_play::SelectionPlayback::Pause,
                crossfade: kithara_play::CrossfadeSettings {
                    duration: 0.0,
                    ..Default::default()
                },
            },
        )
        .expect_err("must error");
    assert!(matches!(err, PlayError::ItemConsumed { item: refused } if refused == item));
    assert_eq!(
        player.current_item(),
        None,
        "bookkeeping must not move on a failed select"
    );
}

#[kithara::test]
fn select_rejects_an_invalid_crossfade() {
    let player = player();
    let err = player
        .select_with_crossfade(
            TrackId::allocate(),
            None,
            SelectTransition {
                playback: kithara_play::SelectionPlayback::Pause,
                crossfade: kithara_play::CrossfadeSettings {
                    duration: -1.0,
                    ..Default::default()
                },
            },
        )
        .expect_err("invalid crossfade must be rejected");
    assert!(matches!(
        err,
        PlayError::InvalidParameter { ref name, value }
            if name == "crossfade.duration" && value == -1.0
    ));
}

/// A player with nothing loaded still owns the position it is handed;
/// discarding it is what makes a restored position play from zero.
#[kithara::test]
fn seek_seconds_without_slot_holds_the_start_position() {
    let player = player();

    let outcome = player.seek_seconds(12.0).expect("must accept");

    assert!(matches!(
        outcome,
        SeekOutcome::Landed { target, landed_at }
            if target == Duration::from_secs(12) && landed_at == target
    ));
    assert_eq!(player.position_seconds(), Some(12.0));
}

/// The held target is the latest one handed over, not the first: a host
/// resets to zero before it restores a stored position.
#[kithara::test]
fn held_start_position_keeps_the_latest_target() {
    let player = player();

    player.seek_seconds(0.0).expect("must accept");
    player.seek_seconds(30.0).expect("must accept");

    assert_eq!(player.position_seconds(), Some(30.0));
}

/// The held target names a place in the queued item, so it must not
/// outlive the queue and land on whatever is seeded next.
#[kithara::test]
fn clearing_the_queue_drops_the_held_start_position() {
    let player = player();
    player.seek_seconds(30.0).expect("must accept");

    player.remove_all_items();

    assert_eq!(player.position_seconds(), None);
}
