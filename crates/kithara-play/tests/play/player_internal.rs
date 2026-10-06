#![cfg(not(target_arch = "wasm32"))]
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::cast_lossless,
    reason = "test fixture values are small positive integers/floats"
)]

use std::sync::atomic::{AtomicU64, Ordering};

use kithara_audio::{
    ConsumerWakeMode,
    mock::{MockReader, TestPcmReader},
};
use kithara_events::{EventBus, EventReceiver, TrackId};
use kithara_platform::sync::{Arc, Mutex};
use kithara_play::{
    AllocatedSlot, Cmd, CrossfadeSettings, NodeInputs, PlayError, PlayWorker, PlayWorkerConfig,
    PlayerConfig, PlayerEvent, PlayerImpl, PlayerStatus, Reply, Resource, SeekOutcome,
    SelectionPlayback, SessionBinding, SessionDispatcher, SessionSampleRate, SharedEq, SlotId,
    SuccessorLink, bridge::slot_channels,
};
use kithara_test_fixtures::integration_fixtures::constant_half;
use kithara_test_utils::{
    bufpool::{TestPools, pools},
    kithara,
};

use crate::support::{AUDIO_SPEC, SAMPLE_RATE};

/// A resource whose src carries `label`, so concurrent items in one test
/// stay distinguishable in failure output.
fn make_tagged_resource(
    constant_half: &'static [u8],
    label: &'static str,
    duration_secs: f64,
) -> Resource {
    Resource::from_reader(
        TestPcmReader::with_pcm(AUDIO_SPEC, duration_secs, constant_half),
        Some(Arc::from(format!("memory://{label}"))),
    )
}

struct FixtureSession {
    next_player: AtomicU64,
    next_slot: AtomicU64,
    nodes: Mutex<Vec<NodeInputs>>,
}

impl FixtureSession {
    fn new() -> Self {
        Self {
            next_player: AtomicU64::new(1),
            next_slot: AtomicU64::new(0),
            nodes: Mutex::default(),
        }
    }
}

impl SessionDispatcher<TestPools> for FixtureSession {
    fn exec(&self, cmd: Cmd<TestPools>) -> Result<Reply, PlayError> {
        let reply = match cmd {
            Cmd::RegisterPlayer { .. } => {
                Reply::PlayerRegistered(kithara_play::session::RegisteredPlayer {
                    id: self.next_player.fetch_add(1, Ordering::Relaxed),
                    eq: SharedEq::new(10),
                })
            }
            Cmd::AllocateSlot { .. } => {
                let slot = SlotId::new(self.next_slot.fetch_add(1, Ordering::Relaxed));
                let (inputs, control) = slot_channels(SharedEq::new(10));
                self.nodes.lock().push(inputs);
                Reply::SlotAllocated(Box::new(AllocatedSlot::new(control, slot)))
            }
            Cmd::QuerySampleRate => {
                Reply::SampleRate(SessionSampleRate::new(None, SAMPLE_RATE.get()))
            }
            _ => Reply::Ok,
        };
        Ok(reply)
    }

    fn consumer_wake_mode(&self) -> ConsumerWakeMode {
        ConsumerWakeMode::RealtimeDeferred
    }
}

fn make_fixture_player(crossfade_duration: f32) -> (PlayerImpl<TestPools>, Arc<FixtureSession>) {
    let bus = EventBus::default();
    let session = Arc::new(FixtureSession::new());
    let player_config = PlayerConfig::builder()
        .bus(bus)
        .crossfade_duration(crossfade_duration)
        .sample_rate(SAMPLE_RATE)
        .worker(PlayWorker::new(PlayWorkerConfig::builder(pools()).build()))
        .session(SessionBinding::new(
            Arc::clone(&session) as Arc<dyn SessionDispatcher<TestPools>>,
            SAMPLE_RATE,
        ))
        .build();
    let player = PlayerImpl::new(player_config);
    (player, session)
}

/// A player whose engine and slot are up, holding nothing yet.
fn prepared_player(crossfade_duration: f32) -> PlayerImpl<TestPools> {
    let (player, _session) = make_fixture_player(crossfade_duration);
    player.ensure_engine_started().unwrap();
    player.ensure_slot().unwrap();
    player
}

/// A prepared player leading `first`, with `second` armed behind it over
/// `link`.
fn deck_with_armed(
    constant_half: &'static [u8],
    crossfade_duration: f32,
    link: SuccessorLink,
) -> (PlayerImpl<TestPools>, TrackId, TrackId) {
    let player = prepared_player(crossfade_duration);
    let (first, second) = (TrackId::allocate(), TrackId::allocate());
    player
        .select(
            first,
            Some(make_tagged_resource(constant_half, "first", 0.05)),
            SelectionPlayback::Play,
        )
        .expect("select the first item");
    player
        .arm_next(
            second,
            make_tagged_resource(constant_half, "second", 0.05),
            link,
        )
        .expect("arm the second item");
    (player, first, second)
}

fn drain_player_events(
    player: &PlayerImpl<TestPools>,
    rx: &mut EventReceiver<PlayerEvent>,
) -> Vec<PlayerEvent> {
    use kithara_platform::tokio::sync::broadcast::error::TryRecvError;
    player.process_notifications();
    let mut events = Vec::new();
    loop {
        match rx.try_recv().map(|env| env.event) {
            Ok(event) => events.push(event),
            Err(TryRecvError::Empty | TryRecvError::Closed) => break,
            Err(TryRecvError::Lagged(_)) => continue,
        }
    }
    events
}

#[kithara::test(tokio)]
#[case(false)]
#[case(true)]
async fn player_remove_all_resets_state(constant_half: &'static [u8], #[case] with_item: bool) {
    let player = prepared_player(0.0);
    if with_item {
        player
            .select(
                TrackId::allocate(),
                Some(make_tagged_resource(constant_half, "item-1", 0.05)),
                SelectionPlayback::Pause,
            )
            .expect("select an item");
        assert!(player.current_item().is_some());
    }
    player.remove_all_items();
    assert_eq!(player.current_item(), None);
    assert_eq!(player.status(), PlayerStatus::Unknown);
}

#[kithara::test]
fn replay_same_item_does_not_re_emit_current_item_changed(constant_half: &'static [u8]) {
    let player = prepared_player(0.0);
    let mut rx = player.subscribe();

    player
        .select(
            TrackId::allocate(),
            Some(make_tagged_resource(constant_half, "item-1", 0.05)),
            SelectionPlayback::Play,
        )
        .expect("select the item");
    let first = drain_player_events(&player, &mut rx);
    let first_count = first
        .iter()
        .filter(|e| matches!(e, PlayerEvent::CurrentItemChanged { .. }))
        .count();
    assert_eq!(
        first_count, 1,
        "selecting the item announces it once: {first:?}"
    );

    player.play();
    let second = drain_player_events(&player, &mut rx);
    let second_count = second
        .iter()
        .filter(|e| matches!(e, PlayerEvent::CurrentItemChanged { .. }))
        .count();
    assert_eq!(
        second_count, 0,
        "resuming the same item must not re-announce CurrentItemChanged: {second:?}"
    );
}

/// A selected resource never passes `ConfigPrep`, so adoption into the
/// real-time arena is the only place a session wake policy can reach it. A
/// reader left on the direct-consumer default publishes its reader events
/// inline from the audio callback.
#[kithara::test(tokio)]
async fn a_selected_resource_adopts_the_session_wake_mode() {
    let player = prepared_player(0.0);
    let (reader, recorded) = MockReader::wake_mode_tracking(AUDIO_SPEC);

    player
        .select(
            TrackId::allocate(),
            Some(Resource::from_reader(reader, None)),
            SelectionPlayback::Play,
        )
        .expect("select the item");

    let applied = *recorded.lock();
    assert_eq!(
        applied,
        Some(ConsumerWakeMode::RealtimeDeferred),
        "adoption must apply the session wake mode to a selected resource"
    );
}

#[kithara::test]
fn re_selecting_the_current_item_does_not_re_announce(constant_half: &'static [u8]) {
    // Re-selecting the item the deck already leads (e.g. while paused) must
    // not re-announce: announce gates on identity, not on calls.
    let player = prepared_player(0.0);
    let item = TrackId::allocate();
    let mut rx = player.subscribe();

    player
        .select(
            item,
            Some(make_tagged_resource(constant_half, "item-1", 0.05)),
            SelectionPlayback::Play,
        )
        .expect("select the item");
    let _ = drain_player_events(&player, &mut rx);

    player
        .select(item, None, SelectionPlayback::Pause)
        .expect("re-select the current item");
    let after = drain_player_events(&player, &mut rx);
    let announces = after
        .iter()
        .filter(|e| matches!(e, PlayerEvent::CurrentItemChanged { .. }))
        .count();
    assert_eq!(
        announces, 0,
        "re-selecting the current item must not re-announce: {after:?}"
    );
}

#[kithara::test]
fn selecting_another_item_announces_it(constant_half: &'static [u8]) {
    let player = prepared_player(0.0);
    let (first, second) = (TrackId::allocate(), TrackId::allocate());
    let mut rx = player.subscribe();

    player
        .select(
            first,
            Some(make_tagged_resource(constant_half, "item-1", 0.05)),
            SelectionPlayback::Play,
        )
        .expect("select the first item");
    let _ = drain_player_events(&player, &mut rx);

    player
        .select(
            second,
            Some(make_tagged_resource(constant_half, "item-2", 0.05)),
            SelectionPlayback::Play,
        )
        .expect("select the second item");
    let after = drain_player_events(&player, &mut rx);
    assert!(
        matches!(
            after
                .iter()
                .filter(|e| matches!(e, PlayerEvent::CurrentItemChanged { .. }))
                .collect::<Vec<_>>()
                .as_slice(),
            [PlayerEvent::CurrentItemChanged { item: Some(announced) }] if *announced == second
        ),
        "selecting another item must announce it once: {after:?}"
    );
    assert_eq!(player.current_item(), Some(second));
}

#[kithara::test]
fn arm_next_arms_the_item(constant_half: &'static [u8]) {
    let (player, _first, second) = deck_with_armed(constant_half, 0.0, SuccessorLink::Gapless);
    assert_eq!(player.armed_next(), Some(second));
}

#[kithara::test]
fn seek_seconds_updates_position_optimistically() {
    let (player, _session) = make_fixture_player(0.0);
    player.ensure_engine_started().unwrap();
    player.ensure_slot().unwrap();

    let outcome = player.seek_seconds(54.689_879_542).expect("seek must land");

    assert!(matches!(outcome, SeekOutcome::Landed { .. }));
    assert_eq!(player.position_seconds(), Some(54.689_879_542));
}

#[kithara::test]
fn arm_next_idempotent_for_the_armed_item(constant_half: &'static [u8]) {
    let (player, _first, second) = deck_with_armed(constant_half, 0.0, SuccessorLink::Gapless);

    player
        .arm_next(
            second,
            make_tagged_resource(constant_half, "second-again", 0.05),
            SuccessorLink::Gapless,
        )
        .expect("arming the armed item again succeeds");
    assert_eq!(player.armed_next(), Some(second));
}

#[kithara::test]
fn arm_next_replaces_a_previously_armed_item(constant_half: &'static [u8]) {
    let (player, _first, _second) = deck_with_armed(constant_half, 0.0, SuccessorLink::Gapless);
    let third = TrackId::allocate();

    player
        .arm_next(
            third,
            make_tagged_resource(constant_half, "third", 0.05),
            SuccessorLink::Gapless,
        )
        .expect("arm the third item");
    assert_eq!(player.armed_next(), Some(third));
}

#[kithara::test]
fn commit_next_of_another_item_returns_typed_error(constant_half: &'static [u8]) {
    let (player, _first, second) = deck_with_armed(constant_half, 1.0, SuccessorLink::Fade);
    let other = TrackId::allocate();

    let err = player
        .commit_next(other, CrossfadeSettings::default())
        .expect_err("mismatch");
    assert!(matches!(
        err,
        PlayError::ArmedItemMismatch { requested, armed } if requested == other && armed == second
    ));
}

#[kithara::test]
fn commit_next_makes_the_successor_current_and_announces_it(constant_half: &'static [u8]) {
    let (player, _first, second) = deck_with_armed(constant_half, 1.0, SuccessorLink::Fade);
    let mut rx: EventReceiver<PlayerEvent> = player.subscribe();

    player
        .commit_next(second, CrossfadeSettings::default())
        .unwrap();
    assert_eq!(player.current_item(), Some(second));
    assert_eq!(player.armed_next(), None, "armed clears after commit");

    let announced = std::iter::from_fn(|| rx.try_recv().ok().map(|env| env.event)).any(
        |event| matches!(event, PlayerEvent::CurrentItemChanged { item: Some(item) } if item == second),
    );
    assert!(announced, "commit_next must publish CurrentItemChanged");
}

#[kithara::test]
fn commit_next_idempotent_when_already_activated(constant_half: &'static [u8]) {
    let (player, _first, second) = deck_with_armed(constant_half, 1.0, SuccessorLink::Fade);

    player
        .commit_next(second, CrossfadeSettings::default())
        .unwrap();
    player
        .commit_next(second, CrossfadeSettings::default())
        .unwrap();
    assert_eq!(player.current_item(), Some(second));
}

#[kithara::test]
fn unarm_next_clears_an_armed_successor(constant_half: &'static [u8]) {
    let (player, first, _second) = deck_with_armed(constant_half, 0.0, SuccessorLink::Gapless);

    player.unarm_next();
    assert_eq!(player.armed_next(), None);
    assert_eq!(player.current_item(), Some(first));
}

#[kithara::test]
fn unarm_next_preserves_activated_current(constant_half: &'static [u8]) {
    let (player, _first, second) = deck_with_armed(constant_half, 1.0, SuccessorLink::Fade);
    player
        .commit_next(second, CrossfadeSettings::default())
        .unwrap();
    player.unarm_next();
    assert_eq!(player.armed_next(), None);
    assert_eq!(player.current_item(), Some(second));
}

#[kithara::test]
fn selecting_another_item_unarms_the_successor(constant_half: &'static [u8]) {
    let (player, _first, _second) = deck_with_armed(constant_half, 1.0, SuccessorLink::Fade);
    let third = TrackId::allocate();

    player
        .select(
            third,
            Some(make_tagged_resource(constant_half, "third", 0.05)),
            SelectionPlayback::Play,
        )
        .unwrap();

    assert_eq!(player.armed_next(), None, "select must unarm");
    assert_eq!(player.current_item(), Some(third));
}

/// Selecting the armed item without a resource promotes the armed track
/// instead of loading it again: the deck already holds its audio.
#[kithara::test]
fn selecting_the_armed_item_promotes_it(constant_half: &'static [u8]) {
    let (player, _first, second) = deck_with_armed(constant_half, 1.0, SuccessorLink::Fade);

    player
        .select(second, None, SelectionPlayback::Play)
        .unwrap();
    player.process_notifications();

    assert_eq!(player.current_item(), Some(second));
    assert_eq!(player.armed_next(), None, "armed track consumed by select");
}
