use kithara_audio::{DecodeErrorKind, TrackFailureKind};
use kithara_command::{ChannelConfig, Seq, channel};
use kithara_events::TrackId;
use kithara_platform::time::Duration;
use kithara_play::{
    Bound, DeckEvent, Outbox, PlayError, Player, PlayerConfig, Settled, Slot, Track, TrackCommand,
    TrackFactory, TrackReceipt, TrackSettings, TrackSnapshot, TrackStatus as PlayerTrackStatus,
};
use kithara_render::bridge::PlaybackFault;
use kithara_signal::SessionFrame;
use kithara_test_utils::kithara;

use super::*;
use crate::{
    QueueConfig, QueueSettings, TrackSource, queue::slots::Active, test_pools::TestPools,
    track::TrackRecord,
};

struct SnapshotTrack(TrackSnapshot);
struct SnapshotFactory;

impl TrackFactory<TestPools> for SnapshotFactory {
    type Track = SnapshotTrack;
    fn track(&self, config: PlayerConfig) -> Result<Self::Track, PlayError> {
        Ok(SnapshotTrack(TrackSnapshot {
            item: config.item,
            slot: config.slot,
            status: PlayerTrackStatus::Loaded,
            speed: 1.0,
            position: Duration::ZERO,
            duration: None,
            abr: None,
            metadata: Default::default(),
        }))
    }
}

impl Player<TestPools> for SnapshotTrack {
    type Command = TrackCommand<TestPools>;
    type Snapshot = TrackSnapshot;
    fn entry(&self, bound: Bound) -> SessionFrame {
        match bound {
            Bound::AtOrAfter(at) | Bound::AtOrBefore(at) => at,
        }
    }
    fn apply(
        &mut self,
        _command: Self::Command,
        _out: &mut Outbox<'_, TestPools>,
    ) -> Result<Option<Seq>, PlayError> {
        Ok(None)
    }
    fn settle(
        &mut self,
        _receipt: TrackReceipt<'_, TestPools>,
        _out: &mut Outbox<'_, TestPools>,
    ) -> Settled {
        Settled::Pending
    }
    fn tick(&mut self, _now: SessionFrame, _out: &mut Outbox<'_, TestPools>) {}
    fn snapshot(&self) -> Self::Snapshot {
        self.0.clone()
    }
}

impl Track<TestPools> for SnapshotTrack {
    fn projected(&self) -> TrackSettings {
        TrackSettings::default()
    }
}

type TestQueue = Queue<TestPools, SnapshotFactory>;

fn selected_second() -> (TestQueue, TrackId, TrackId) {
    let config = QueueConfig {
        factory: SnapshotFactory,
        mixer: DeckMixerConfig::default(),
        settings: QueueSettings::default(),
        preload_lead: Duration::from_millis(3_500),
        track: TrackSettings::default(),
        prep: None,
        cancel: None,
        store: None,
        runtime: None,
        should_autoplay: false,
        max_history_size: 100,
        playback_order: Default::default(),
        action_at_item_end: ActionAtItemEnd::None,
    };
    let mut queue = Queue::new(config);
    let first = TrackId::allocate();
    let second = TrackId::allocate();
    for (id, slot, role) in [
        (first, Slot::new(1), Role::Outgoing),
        (second, Slot::new(0), Role::Current),
    ] {
        queue.tracks.records_mut().push(TrackRecord::new(
            id,
            "repeated".to_owned(),
            TrackSource::from("https://example.com/repeated.mp3"),
        ));
        queue.tracks.set_status(id, TrackStatus::Loaded);
        let mut track = queue
            .config
            .factory
            .track(PlayerConfig {
                item: id,
                slot,
                settings: TrackSettings::default(),
            })
            .expect("snapshot track");
        track.0.status = PlayerTrackStatus::Playing {
            since: SessionFrame::new(0),
        };
        queue.active.push(Active {
            item: id,
            slot,
            track,
            role,
            load: None,
        });
    }
    queue.current = Some(second);
    queue.publish();
    (queue, first, second)
}

fn report(queue: &mut TestQueue, event: DeckEvent) {
    let (mut deck, _deck_inbox) = channel(ChannelConfig::builder().build());
    let (mut dispatcher, _dispatcher_inbox) = channel(ChannelConfig::builder().build());
    queue.item_event(event, &mut Outbox::new(&mut deck, &mut dispatcher));
    queue.publish();
}

fn accepted_failure(queue: &mut TestQueue, id: TrackId, fault: PlaybackFault) -> DeckEvent {
    let active = queue
        .active
        .iter_mut()
        .find(|active| active.item == id)
        .expect("active fixture item");
    let at = SessionFrame::new(7);
    active.track.0.status = PlayerTrackStatus::Failed { at, fault };
    DeckEvent::Failed {
        slot: active.slot,
        at,
        fault,
    }
}

fn decode_fault() -> PlaybackFault {
    PlaybackFault::Source(TrackFailureKind::Decode {
        kind: DecodeErrorKind::InvalidData,
    })
}

#[kithara::test]
fn leading_failure_marks_the_played_entry_when_sources_repeat() {
    let (mut queue, first, second) = selected_second();
    let event = accepted_failure(&mut queue, second, decode_fault());
    report(&mut queue, event);
    assert!(
        !matches!(
            queue.track(first).map(|entry| entry.status),
            Some(TrackStatus::Failed(_))
        ),
        "an event for the second repeated source must not fail the first entry"
    );
    assert!(
        matches!(
            queue.track(second).map(|entry| entry.status),
            Some(TrackStatus::Failed(_))
        ),
        "the entry named by the player event must be failed"
    );
}

#[kithara::test]
#[case::invalid_data(PlaybackFault::Source(TrackFailureKind::Decode { kind: DecodeErrorKind::InvalidData }))]
#[case::unsupported_codec(PlaybackFault::Source(TrackFailureKind::Decode { kind: DecodeErrorKind::UnsupportedCodec }))]
#[case::direct_io(PlaybackFault::Source(TrackFailureKind::Decode { kind: DecodeErrorKind::Io }))]
#[case::output_rate(PlaybackFault::OutputRateMismatch)]
#[case::output_range(PlaybackFault::OutputRangeUnavailable)]
fn a_leading_failure_records_the_fault_the_player_reported(#[case] fault: PlaybackFault) {
    let (mut queue, first, second) = selected_second();
    let mut events = queue.subscribe::<QueueEvent>();
    let event = accepted_failure(&mut queue, second, fault);
    report(&mut queue, event);
    let Some(TrackStatus::Failed(reason)) = queue.track(second).map(|entry| entry.status) else {
        panic!("the entry named by the player event must be failed");
    };
    let published = std::iter::from_fn(|| events.try_recv().ok())
        .filter_map(|envelope| match envelope.event {
            QueueEvent::TrackLoadFailed {
                id,
                reason,
                auto_skipped,
            } => Some((id, reason, auto_skipped)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(published, [(second, reason.clone(), false)]);
    assert_eq!(
        reason,
        fault.to_string(),
        "status and event must report the real cause without fabricating an engine failure"
    );
    assert!(
        !matches!(
            queue.track(first).map(|entry| entry.status),
            Some(TrackStatus::Failed(_))
        ),
        "a repeated URI does not make the first entry the failed item"
    );
    report(&mut queue, event);
    assert!(
        std::iter::from_fn(|| events.try_recv().ok())
            .all(|envelope| !matches!(envelope.event, QueueEvent::TrackLoadFailed { .. }))
    );
}

#[kithara::test]
#[case::stale(false)]
#[case::paused(true)]
fn a_stale_or_paused_failure_cannot_change_status_or_publish_a_failure(#[case] paused: bool) {
    let (mut queue, first, second) = selected_second();
    let reported = if paused { second } else { first };
    if paused {
        let active = queue
            .active
            .iter_mut()
            .find(|active| active.item == second)
            .expect("setup allocates a player slot");
        assert_eq!(active.role, Role::Current);
        active.track.0.status = PlayerTrackStatus::Paused { at: Duration::ZERO };
        assert!(
            matches!(active.track.0.status, PlayerTrackStatus::Paused { .. }),
            "setup must pause the active player"
        );
        assert_eq!(queue.current().map(|entry| entry.id), Some(second));
    }
    let before = queue
        .track(reported)
        .expect("the reported entry exists")
        .status;
    let mut events = queue.subscribe::<QueueEvent>();
    let slot = queue
        .active
        .iter()
        .find(|active| active.item == reported)
        .expect("reported player slot")
        .slot;
    report(
        &mut queue,
        DeckEvent::Failed {
            slot,
            at: SessionFrame::new(7),
            fault: decode_fault(),
        },
    );
    assert_eq!(queue.current().map(|entry| entry.id), Some(second));
    assert_eq!(
        queue.track(reported).expect("the entry survives").status,
        before
    );
    assert!(
        std::iter::from_fn(|| events.try_recv().ok())
            .all(|envelope| !matches!(envelope.event, QueueEvent::TrackLoadFailed { .. })),
        "an ignored item failure must not publish a queue failure"
    );
}

#[kithara::test]
fn background_end_and_failure_leave_the_current_entry_untouched() {
    let (mut queue, background, current) = selected_second();
    report(
        &mut queue,
        DeckEvent::Ended {
            slot: Slot::new(1),
            at: SessionFrame::new(7),
        },
    );
    let event = accepted_failure(&mut queue, background, decode_fault());
    report(&mut queue, event);
    assert_eq!(queue.current().map(|entry| entry.id), Some(current));
    assert!(
        !matches!(
            queue.track(background).map(|entry| entry.status),
            Some(TrackStatus::Failed(_))
        ),
        "a background failure must not fail its queue entry"
    );
}

#[kithara::test]
fn failed_playback_stopped_notification_carries_the_role() {
    let (mut queue, background, _) = selected_second();
    let event = accepted_failure(&mut queue, background, decode_fault());
    report(&mut queue, event);
    let active = queue
        .active
        .iter()
        .find(|active| active.item == background)
        .expect("outgoing player remains resident");
    assert_eq!(active.role, Role::Outgoing);
    assert!(matches!(
        active.track.snapshot().status,
        PlayerTrackStatus::Failed {
            fault: PlaybackFault::Source(TrackFailureKind::Decode {
                kind: DecodeErrorKind::InvalidData
            }),
            ..
        }
    ));
}
