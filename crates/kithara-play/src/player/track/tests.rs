use kithara_assets::AssetStore;
use kithara_audio::{AudioObserverSlot, mock::TestPcmReader};
use kithara_command::{ChannelConfig, Inbox, channel};
use kithara_platform::sync::Arc;
use kithara_render::{
    LaneFrame, LoadRefusal,
    bridge::DeckEvent,
    rt::{DeckMixerConfig, track::PlayerResource},
};
use kithara_signal::AudioSpec;
use kithara_test_utils::kithara;
use kithara_warp::{SpeedCurve, StretchKind};

use super::*;
use crate::{
    Resource, ResourceConfig, ResourceSrc,
    mock::{self, DeckRig},
    test_pools::{TestPools, pools},
};

const A: Slot = Slot::new(0);
const B: Slot = Slot::new(1);

type Rig = DeckRig<TestPools>;
type Track = PlayerImpl<TestPools>;

fn rig() -> Rig {
    DeckRig::new(DeckMixerConfig::default())
}

fn frame(value: i64) -> SessionFrame {
    SessionFrame::new(value)
}

fn track(slot: Slot) -> Track {
    let settings = TrackSettings::builder()
        .speed(1.0)
        .keylock(false)
        .backend(StretchKind::default())
        .build();
    PlayerImpl::new(PlayerConfig {
        item: TrackId::allocate(),
        slot,
        settings,
    })
    .expect("unity speed is a valid track speed")
}

fn item() -> ResourceLoad<TestPools> {
    let src = ResourceSrc::parse("https://example.com/song.mp3").expect("valid test source");
    let config = ResourceConfig::for_src(src)
        .store(AssetStore::builder(pools()).build())
        .build();
    ResourceLoad::new(config, Box::new(AudioObserverSlot::default().relay()))
}

fn lane() -> (Sender<LaneProtocol>, Inbox<LaneProtocol>) {
    channel(ChannelConfig::builder().build())
}

/// What the dispatcher opens for a track of `src`.
fn opened(src: &str, lane: Option<Sender<LaneProtocol>>) -> OpenedTrack {
    let reader = TestPcmReader::new(AudioSpec::new(2, mock::SAMPLE_RATE), 0.01);
    let pcm = PlayerResource::new(
        Resource::from_reader(reader, None).into(),
        Arc::from(src),
        &pools(),
    )
    .expect("player resource fits the test pool budget");
    OpenedTrack {
        pcm: Box::new(pcm),
        lane,
        duration: Some(Duration::from_millis(10)),
        abr: None,
        metadata: TrackMetadata::default(),
    }
}

/// The commands the lane takes in its next block, in order.
fn lane_commands(inbox: &mut Inbox<LaneProtocol>) -> Vec<LaneCommand> {
    inbox.drain();
    let mut commands = Vec::new();
    while let Some(due) = inbox.next_due(LaneFrame(0), 1) {
        commands.extend(due.commands().iter().copied());
        due.apply(());
    }
    commands
}

/// Hands every receipt to `track`, in order, and returns what it reported.
fn settle(track: &mut Track, rig: &mut Rig, receipts: &[Receipt<DeckProtocol>]) -> Vec<Settled> {
    receipts
        .iter()
        .map(|receipt| track.settle(TrackReceipt::Deck(receipt), &mut rig.outbox()))
        .collect()
}

/// Asks the dispatcher to open a track that stands at `position` once attached.
fn load(track: &mut Track, rig: &mut Rig, position: Position) -> Seq {
    track
        .apply(
            TrackCommand::Load {
                item: item(),
                position,
            },
            &mut rig.outbox(),
        )
        .expect("the dispatcher has room")
        .expect("a load goes out on its own")
}

/// A track of `src` in `slot`, its consumer attached on frame 0.
fn loaded(rig: &mut Rig, slot: Slot, src: &str) -> (Track, Inbox<LaneProtocol>) {
    let (sender, inbox) = lane();
    let mut track = track(slot);
    load(&mut track, rig, Position::ZERO);
    let receipt = rig
        .open(Ok(opened(src, Some(sender))))
        .expect("the load reached the dispatcher");
    track.settle(TrackReceipt::Loaded(receipt), &mut rig.outbox());
    let receipts = rig.block(frame(0), 0.0);
    settle(&mut track, rig, &receipts);
    assert_eq!(track.snapshot().status, TrackStatus::Loaded);
    (track, inbox)
}

#[kithara::test]
fn a_load_opens_its_source_once_and_attaches_on_the_next_block() {
    let mut rig = rig();
    let mut track = track(A);

    let seq = load(&mut track, &mut rig, Position::ZERO);
    assert_eq!(track.snapshot().status, TrackStatus::Loading);
    let receipt = rig
        .open(Ok(opened("track", None)))
        .expect("the load reached the dispatcher");
    assert!(rig.open(Ok(opened("track", None))).is_none(), "one open per load");
    assert!(matches!(
        track.settle(TrackReceipt::Loaded(receipt), &mut rig.outbox()),
        Settled::Pending
    ));
    assert_eq!(rig.mixer.held(A), None, "the attach waits for the block");

    let receipts = rig.block(frame(0), 0.0);
    let settled = settle(&mut track, &mut rig, &receipts);

    assert!(
        matches!(settled.as_slice(), [Settled::Applied { seq: answered, .. }] if *answered == seq),
        "the attach answers the load: {settled:?}"
    );
    assert_eq!(rig.mixer.held(A), Some("track"));
    assert_eq!(track.snapshot().status, TrackStatus::Loaded);
}

#[kithara::test]
fn a_track_loaded_at_a_position_stands_there_once_attached() {
    let mut rig = rig();
    let mut track = track(A);
    load(&mut track, &mut rig, Position::from_secs(3));
    let receipt = rig.open(Ok(opened("track", None))).expect("one open");
    track.settle(TrackReceipt::Loaded(receipt), &mut rig.outbox());

    let receipts = rig.block(frame(0), 0.0);
    settle(&mut track, &mut rig, &receipts);

    assert!(
        matches!(
            receipts[0].batch().commands.as_slice(),
            [DeckPart::Seek { slot, seconds, .. }] if *slot == A && *seconds == 3.0
        ),
        "{:?}",
        receipts[0].batch().commands
    );
    assert_eq!(track.snapshot().position, Position::from_secs(3));
}

#[kithara::test]
fn a_refused_open_leaves_the_track_idle() {
    let mut rig = rig();
    let mut track = track(A);
    let seq = load(&mut track, &mut rig, Position::ZERO);

    let receipt = rig
        .open(Err(LoadRefusal::Cancelled))
        .expect("the load reached the dispatcher");
    let settled = track.settle(TrackReceipt::Loaded(receipt), &mut rig.outbox());

    assert!(
        matches!(settled, Settled::Rejected { seq: answered, .. } if answered == seq),
        "{settled:?}"
    );
    assert_eq!(track.snapshot().status, TrackStatus::Idle);
    assert!(rig.block(frame(0), 0.0).is_empty(), "nothing went to the deck");
}

#[kithara::test]
fn play_sounds_from_its_frame_and_pause_reports_where_it_stopped() {
    let mut rig = rig();
    let (mut track, _lane) = loaded(&mut rig, A, "track");

    let play = track
        .apply(TrackCommand::Play { at: When::At(frame(4_096)) }, &mut rig.outbox())
        .expect("the deck has room")
        .expect("a play goes out on its own");
    assert!(rig.block(frame(0), 0.0).is_empty(), "the start waits for its frame");
    let receipts = rig.block(frame(4_096), 0.0);
    let settled = settle(&mut track, &mut rig, &receipts);
    assert!(
        matches!(settled.as_slice(), [Settled::Applied { seq, at }] if *seq == play && *at == frame(4_096)),
        "{settled:?}"
    );
    assert_eq!(
        track.snapshot().status,
        TrackStatus::Playing { since: frame(4_096) }
    );

    track
        .apply(TrackCommand::Pause { at: When::At(frame(8_192)) }, &mut rig.outbox())
        .expect("the deck has room");
    let receipts = rig.block(frame(8_192), 1.5);
    settle(&mut track, &mut rig, &receipts);

    let snapshot = track.snapshot();
    assert_eq!(snapshot.status, TrackStatus::Paused { at: Position::from_secs_f64(1.5) });
    assert_eq!(snapshot.position, Position::from_secs_f64(1.5));
}

#[kithara::test]
fn a_track_played_after_another_chains_from_its_slot() {
    let mut rig = rig();
    let (mut leading, _) = loaded(&mut rig, A, "leading");
    let (mut following, _) = loaded(&mut rig, B, "following");

    following
        .apply(TrackCommand::PlayAfter { track: A }, &mut rig.outbox())
        .expect("the deck has room");
    let receipts = rig.block(frame(0), 0.0);

    assert!(
        matches!(
            receipts.as_slice(),
            [receipt] if matches!(
                receipt.batch().commands.as_slice(),
                [DeckPart::Chain { from, to }] if *from == A && *to == B
            ) && receipt.batch().basis.iter().map(|&(slot, _)| slot).eq([A, B])
        ),
        "{:?}",
        receipts.iter().map(|receipt| &receipt.batch().commands).collect::<Vec<_>>()
    );
    settle(&mut leading, &mut rig, &receipts);
    settle(&mut following, &mut rig, &receipts);
    assert_eq!(leading.snapshot().status, TrackStatus::Loaded);
    assert_eq!(
        following.snapshot().status,
        TrackStatus::Playing { since: frame(0) }
    );
}

#[kithara::test]
fn every_part_a_track_sends_names_its_slot() {
    let mut rig = rig();
    let (mut track, _lane) = loaded(&mut rig, B, "track");
    for command in [
        TrackCommand::Fade {
            at: When::Next,
            settings: CrossfadeSettings::default(),
            dir: FadeDir::In,
        },
        TrackCommand::Seek {
            to: Position::from_secs(1),
        },
        TrackCommand::Release,
    ] {
        track
            .apply(command, &mut rig.outbox())
            .expect("the deck has room");
    }

    let receipts = rig.block(frame(0), 0.0);
    let parts: Vec<_> = receipts
        .iter()
        .flat_map(|receipt| receipt.batch().commands.iter())
        .collect();
    assert!(
        matches!(
            parts.as_slice(),
            [
                DeckPart::Start { slot: started, fade: Fade::Crossfade(_) },
                DeckPart::Seek { slot: sought, .. },
                DeckPart::Released(Released::Pcm { slot: released, .. }),
            ] if [*started, *sought, *released] == [B; 3]
        ),
        "{parts:?}"
    );
    assert!(receipts.iter().all(|receipt| TrackReceipt::<TestPools>::Deck(receipt).names(B)));
}

#[kithara::test]
fn a_release_frees_the_track_once_the_mixer_let_its_consumer_go() {
    let mut rig = rig();
    let (mut track, _lane) = loaded(&mut rig, A, "track");

    track
        .apply(TrackCommand::Release, &mut rig.outbox())
        .expect("the deck has room");
    assert_ne!(track.snapshot().status, TrackStatus::Released, "the detach waits for its receipt");
    let receipts = rig.block(frame(0), 0.0);
    settle(&mut track, &mut rig, &receipts);

    assert_eq!(track.snapshot().status, TrackStatus::Released);
    assert_eq!(rig.mixer.held(A), None);
}

#[kithara::test]
fn an_evicting_track_takes_the_slot_over_on_its_frame() {
    let mut rig = rig();
    let (mut old, _) = loaded(&mut rig, A, "old");
    old.apply(TrackCommand::Play { at: When::Next }, &mut rig.outbox())
        .expect("the deck has room");
    let receipts = rig.block(frame(0), 0.0);
    settle(&mut old, &mut rig, &receipts);

    let mut new = track(A);
    let seq = load(&mut new, &mut rig, Position::ZERO);
    new.apply(TrackCommand::Evict { at: When::At(frame(1_024)) }, &mut rig.outbox())
        .expect("the load is in flight");
    let receipt = rig.open(Ok(opened("new", None))).expect("one open");
    new.settle(TrackReceipt::Loaded(receipt), &mut rig.outbox());
    assert_eq!(rig.mixer.held(A), Some("old"), "the replace waits for its frame");

    let receipts = rig.block(frame(1_024), 0.0);
    settle(&mut old, &mut rig, &receipts);
    let settled = settle(&mut new, &mut rig, &receipts);

    assert!(
        matches!(
            receipts[0].batch().commands.as_slice(),
            [
                DeckPart::Released(Released::Pcm { slot, pcm }),
                DeckPart::Start { slot: started, fade: Fade::Declick },
            ] if *slot == A && &**pcm.src() == "old" && *started == A
        ),
        "{:?}",
        receipts[0].batch().commands
    );
    assert!(
        matches!(settled.as_slice(), [Settled::Applied { seq: answered, at }] if *answered == seq && *at == frame(1_024)),
        "the replace answers the load: {settled:?}"
    );
    assert_eq!(rig.mixer.held(A), Some("new"));
    assert_eq!(old.snapshot().status, TrackStatus::Released);
    assert_eq!(
        new.snapshot().status,
        TrackStatus::Playing { since: frame(1_024) }
    );
}

#[kithara::test]
fn a_track_whose_slot_faded_out_reports_it() {
    let mut rig = rig();
    let (mut track, _lane) = loaded(&mut rig, A, "track");

    track.settle(
        TrackReceipt::Event(DeckEvent::Faded {
            slot: A,
            at: frame(2_048),
        }),
        &mut rig.outbox(),
    );

    assert_eq!(track.snapshot().status, TrackStatus::Faded { at: frame(2_048) });
}

#[kithara::test]
fn a_load_the_deck_has_no_room_for_leaves_the_lane_untouched() {
    let mut rig = rig();
    while rig
        .ring
        .send(
            When::Next,
            Batch {
                basis: Vec::new(),
                commands: Vec::new(),
            },
        )
        .is_ok()
    {}
    let (sender, mut inbox) = lane();
    let mut track = track(A);
    let seq = load(&mut track, &mut rig, Position::ZERO);
    let receipt = rig.open(Ok(opened("track", Some(sender)))).expect("one open");

    let settled = track.settle(TrackReceipt::Loaded(receipt), &mut rig.outbox());
    track
        .apply(
            TrackCommand::Configure(TrackSettingsChange::Speed(2.0), When::Next),
            &mut rig.outbox(),
        )
        .expect("a valid speed");

    assert!(
        matches!(
            settled,
            Settled::Rejected { seq: answered, reason: Rejection::Refused(PlayError::Full("deck")) }
                if answered == seq
        ),
        "{settled:?}"
    );
    assert_eq!(track.snapshot().status, TrackStatus::Idle);
    let commands = lane_commands(&mut inbox);
    assert!(commands.is_empty(), "{commands:?}");
}

#[kithara::test]
fn a_loaded_track_sends_each_change_to_its_lane() {
    let mut rig = rig();
    let (mut track, mut inbox) = loaded(&mut rig, A, "track");

    let sent = track
        .apply(
            TrackCommand::Configure(TrackSettingsChange::Speed(2.0), When::Next),
            &mut rig.outbox(),
        )
        .expect("a valid speed");

    assert!(sent.is_some());
    let commands = lane_commands(&mut inbox);
    assert!(
        matches!(
            commands.as_slice(),
            [LaneCommand::SetSpeed(SpeedCurve::Constant(changed))] if *changed == 2.0
        ),
        "{commands:?}"
    );
    assert!(
        (track.snapshot().speed - 1.0).abs() < f32::EPSILON,
        "the speed shows once the lane applied it"
    );
    track.tick(frame(0), &mut rig.outbox());
    assert!((track.snapshot().speed - 2.0).abs() < f32::EPSILON);
}

#[kithara::test]
#[case::silent(false)]
#[case::loaded(true)]
fn a_change_at_a_frame_is_refused_as_untimed(#[case] attached: bool) {
    let mut rig = rig();
    let (mut track, mut inbox) = if attached {
        loaded(&mut rig, A, "track")
    } else {
        (track(A), lane().1)
    };

    let refused = track.apply(
        TrackCommand::Configure(TrackSettingsChange::Keylock(true), When::At(frame(4_096))),
        &mut rig.outbox(),
    );

    assert!(matches!(refused, Err(PlayError::Untimed)), "{refused:?}");
    assert!(lane_commands(&mut inbox).is_empty());
    assert!(!track.snapshot().status.eq(&TrackStatus::Released));
}

#[kithara::test]
fn a_change_before_the_load_applies_at_once_and_starts_the_lane_there() {
    let mut rig = rig();
    let mut track = track(A);

    let sent = track
        .apply(
            TrackCommand::Configure(TrackSettingsChange::Speed(1.25), When::Next),
            &mut rig.outbox(),
        )
        .expect("a valid speed");

    assert!(sent.is_none(), "no lane to send to yet");
    assert!((track.snapshot().speed - 1.25).abs() < f32::EPSILON);
}


#[kithara::test]
fn failed_deck_event_preserves_item_identity_and_the_first_terminal_cause() {
    use kithara_render::bridge::PlaybackFault;
    use kithara_audio::TrackFailureKind;
    let mut rig = rig();
    let mut player = track(A);
    let item = player.snapshot().item;
    player.status = TrackStatus::Playing { since: frame(0) };
    let fault = PlaybackFault::Source(TrackFailureKind::SourceCancelled);
    player.settle(TrackReceipt::Event(DeckEvent::Failed { slot: A, at: frame(7), fault }), &mut rig.outbox());
    let terminal = player.snapshot();
    assert_eq!(terminal.item, item);
    assert_eq!(terminal.slot, A);
    assert_eq!(terminal.status, TrackStatus::Failed { at: frame(7), fault });
    for event in [
        DeckEvent::Failed { slot: A, at: frame(8), fault: PlaybackFault::Source(TrackFailureKind::ChannelClosed) },
        DeckEvent::Ended { slot: A, at: frame(9) },
        DeckEvent::Failed { slot: B, at: frame(10), fault },
    ] {
        player.settle(TrackReceipt::Event(event), &mut rig.outbox());
        assert_eq!(player.snapshot().item, item);
        assert_eq!(player.snapshot().status, terminal.status);
    }
    assert!(fault.to_string().contains("source cancelled"));
}

#[kithara::test]
#[case::stale(false)]
#[case::paused(true)]
fn stale_or_paused_failure_does_not_change_the_scoped_player(#[case] paused: bool) {
    use kithara_render::bridge::PlaybackFault;
    use kithara_audio::{DecodeErrorKind, TrackFailureKind};
    let mut rig = rig();
    let mut player = track(A);
    player.status = if paused { TrackStatus::Paused { at: Position::ZERO } } else { TrackStatus::Playing { since: frame(8) } };
    let before = player.snapshot();
    player.settle(TrackReceipt::Event(DeckEvent::Failed {
        slot: A, at: frame(7), fault: PlaybackFault::Source(TrackFailureKind::Decode { kind: DecodeErrorKind::InvalidData }),
    }), &mut rig.outbox());
    assert_eq!(player.snapshot().item, before.item);
    assert_eq!(player.snapshot().status, before.status);
}
