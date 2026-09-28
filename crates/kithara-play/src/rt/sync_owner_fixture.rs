//! A real Sync owner and executor for RT unit tests that need a ticket the
//! executor hands over, and the mocked resources those tests render.

use std::{
    num::{NonZeroU32, NonZeroU64, NonZeroUsize},
    sync::atomic::{AtomicUsize, Ordering},
};

use kithara_audio::{
    ReadOutcome as AudioReadOutcome,
    mock::{AudioControlMock, AudioReadMock, AudioSessionMock},
};
use kithara_decode::DecodeError;
use kithara_events::{EventBus, TrackId};
use kithara_platform::{
    CancelScope, CancelToken,
    maybe_send::MaybeSendFuture,
    sync::{Arc, Mutex},
    time::Duration,
    tokio::{
        runtime::{Builder, Handle},
        sync::mpsc,
    },
};
use kithara_signal::{AudioSpec, SessionEpoch, SessionFrame, SourceSpan, TransportRevision};
use kithara_sync::{
    ActivationHead, AlignmentSource, ExecutedGroup, GroupState, LoadedMedia, ParentFact,
    ParentGridUpdate, StagePort, Staged, SyncAdmission, SyncError, SyncExecutionReject,
    SyncExecutionStamp, SyncExecutor, SyncGateBinding, SyncGroup, SyncGroupSnapshot, SyncIntent,
    SyncMember, SyncMode, SyncOperation, SyncReceipt, SyncReceiptAck, SyncRejected, SyncStaged,
    SyncStatusSnapshot, SyncTransition,
    mock::{MemberOwner, ReceiptSinkMock},
};
use kithara_warp::{
    AssetAxis, AssetExtent, AssetFrame, BeatEvidence, BeatGrid, BeatGridId, BeatGridRevision,
    BeatGridSnapshot, BeatGridStamp, BeatGridState, BeatMarker, BeatOrdinal, FrameUncertainty,
    MapAxis, MapPosition, MapSegment, SegmentFacts, SegmentSet, SessionAnchor, SessionBeat,
    WarpPlan,
};
use unimock::{MockFn, Unimock, matching};

use crate::{
    bridge::sync::SyncTicket, resource::Resource, rt::track::PlayerResource, test_pools::pools,
};

struct FixtureGroup(GroupState<Self>);

impl BeatGrid for FixtureGroup {
    delegate::delegate! {
        to self.0 {
            fn id(&self) -> BeatGridId;
            fn snapshot(&self) -> BeatGridSnapshot;
        }
    }
}

impl SyncGroup for FixtureGroup {
    type NestedGroup = Self;

    delegate::delegate! {
        to self.0 {
            fn stage_fact(&self, fact: ParentFact) -> Result<SyncStaged, SyncError>;
            fn apply_staged(&mut self, staged: SyncStaged) -> SyncTransition;
            fn acknowledge(&mut self, receipt: SyncReceipt) -> Result<SyncStatusSnapshot, SyncError>;
            fn status(&self) -> SyncStatusSnapshot;
            fn mode(&self) -> SyncMode;
            fn topology(&self) -> Result<SyncGroupSnapshot, SyncError>;
            fn transact(&mut self, operation: SyncOperation<Self>) -> Result<SyncAdmission, SyncRejected<Self>>;
        }
    }
}

struct FixtureGrid(BeatGridSnapshot);

impl BeatGrid for FixtureGrid {
    delegate::delegate! {
        to self.0 {
            fn id(&self) -> BeatGridId;
            #[call(clone)]
            fn snapshot(&self) -> BeatGridSnapshot;
        }
    }
}

fn source_grid(member: BeatGridId, rate: NonZeroU32) -> BeatGridSnapshot {
    let exact = FrameUncertainty::new(0.0).expect("fixture uncertainty");
    let marker = |frame: u64, ordinal: i64| {
        BeatMarker::new(
            MapPosition::Asset(AssetFrame::new(frame as f64).expect("fixture source frame")),
            Some(BeatOrdinal::new(ordinal)),
            BeatEvidence::Observed,
            exact,
        )
    };
    let segment = MapSegment::new(
        marker(0, 0),
        marker(24_000, 1),
        SegmentFacts::new(BeatEvidence::Observed, exact, None),
    )
    .expect("fixture beat interval");
    let segments = SegmentSet::new(
        MapAxis::Asset(AssetAxis::new(rate, AssetExtent::Bounded(24_000))),
        vec![segment],
    )
    .expect("fixture segment set");
    BeatGridSnapshot::segments(
        member,
        BeatGridRevision::first(),
        BeatGridState::Complete,
        segments,
    )
    .expect("fixture source grid")
}

/// The source interval of the one frame `head` enters at.
fn first_span(head: ActivationHead) -> SourceSpan {
    let cursor = head.activation();
    SourceSpan::new(cursor.source(), cursor.source() + 1, head.source_rate(), 1)
        .expect("one source frame")
        .with_mapping_revision(NonZeroU64::new(u64::from(cursor.revision())))
}

/// What ends a test's wait for its entry: the ticket the executor handed
/// over, or the receipt that rejected it.
type Entered = Result<SyncTicket, SyncReceipt>;

/// Stages the test's one lane at its plan head, decoding `stereo` there,
/// and hands the executor's ticket back to the test.
#[derive(Clone)]
struct FixturePort {
    lane: Arc<Mutex<Option<Box<PlayerResource>>>>,
    runtime: Handle,
    gate: SyncGateBinding,
    entered: mpsc::UnboundedSender<Entered>,
    stereo: [f32; 2],
}

impl StagePort for FixturePort {
    type Item = TrackId;
    type Lane = Box<PlayerResource>;

    fn gate(&self) -> &SyncGateBinding {
        &self.gate
    }

    fn handoff(self, ticket: SyncTicket) -> Result<(), SyncExecutionReject> {
        let _ = self.entered.send(Ok(ticket));
        Ok(())
    }

    fn runtime(&self) -> &Handle {
        &self.runtime
    }

    fn stage(
        self,
        _plan: WarpPlan,
        head: ActivationHead,
        _cancel: CancelToken,
    ) -> impl MaybeSendFuture<Output = Result<Staged<Self::Lane>, SyncExecutionReject>> + 'static
    {
        let lane = self
            .lane
            .lock()
            .take()
            .expect("the fixture stages one lane");
        async move { Ok(Staged::new(lane, self.stereo, first_span(head))) }
    }
}

/// The group owner of the fixture: `member_owner` mints the permit of an
/// installed lane, and a rejection ends the test's wait. The executor drops
/// its sink on whichever thread lets go of it last, so the sink does not
/// verify in drop; a ticket handed over proves both clauses were called.
fn fixture_sink(member_owner: MemberOwner, entered: mpsc::UnboundedSender<Entered>) -> Unimock {
    Unimock::new((
        ReceiptSinkMock::is_bound
            .each_call(matching!())
            .returns(true),
        ReceiptSinkMock::acknowledge
            .each_call(matching!(_))
            .answers_arc(Arc::new(move |_, receipt| match receipt {
                SyncReceipt::Installed(stamp) => {
                    SyncReceiptAck::Installed(member_owner.mint(stamp).expect("exact permit"))
                }
                receipt => {
                    let _ = entered.send(Err(receipt));
                    SyncReceiptAck::Recorded
                }
            })),
    ))
    .no_verify_in_drop()
}

/// The ticket the executor hands over for `lane`, loaded as `media`, after
/// one public owner Enable at output frame 32 prepared from source frame
/// `cue`, decoding `stereo` at its head; with the stamp of that
/// preparation. The member is the one `member_owner` holds, and it mints
/// the permit.
pub(crate) fn entry_ticket(
    member_owner: MemberOwner,
    media: LoadedMedia<TrackId>,
    lane: Box<PlayerResource>,
    stereo: [f32; 2],
    cue: u64,
    rate: NonZeroU32,
    output_transport: Option<TransportRevision>,
) -> (SyncTicket, SyncExecutionStamp) {
    let fixture_runtime = Builder::new_current_thread()
        .build()
        .expect("fixture runtime");
    let member = member_owner.member();
    let gate = member_owner.gate();
    let (entered, mut entries) = mpsc::unbounded_channel();
    let sink = fixture_sink(member_owner, entered.clone());
    let executor = SyncExecutor::new(member, Some(Arc::new(sink)), CancelScope::new(None).token());
    let lane = Arc::new(Mutex::new(Some(lane)));
    let runtime = fixture_runtime.handle().clone();
    executor.load(
        media,
        Some(FixturePort {
            lane,
            runtime,
            gate,
            entered,
            stereo,
        }),
    );
    let group = BeatGridId::allocate().expect("fixture group identity");
    let mut owner = ExecutedGroup::new(
        GroupState::<FixtureGroup>::owning(
            group,
            rate,
            SessionEpoch::new(1),
            SyncMember::Grid {
                alignment: None,
                grid: Box::new(FixtureGrid(source_grid(member, rate))),
            },
        ),
        executor.execution(),
    );
    let parent = BeatGridId::allocate().expect("fixture parent identity");
    let anchor = SessionAnchor::new(SessionFrame::new(32), SessionBeat::default(), 2.0, rate)
        .expect("fixture parent anchor");
    let mut update = ParentGridUpdate::new(
        BeatGridStamp::new(parent, BeatGridRevision::first()),
        SessionEpoch::new(1),
        anchor,
        None,
    );
    if let Some(revision) = output_transport {
        update = update.with_output_transport(revision);
    }
    let staged = owner
        .stage_fact(ParentFact::Segment(update))
        .expect("fixture parent publication");
    owner.apply_staged(staged);
    let admission = owner
        .transact(SyncOperation::Sync {
            target: group,
            load: media.load(),
            transport: TransportRevision::first(),
            source: AlignmentSource::Prepared(AssetFrame::new(cue as f64).expect("fixture cue")),
            activation: SessionFrame::new(32),
            intent: SyncIntent::Enable,
        })
        .expect("public owner Enable");
    let SyncAdmission::StateChanged { transition, .. } = admission else {
        panic!("public Enable must issue a preparation");
    };
    let [preparation] = transition.issued() else {
        panic!("one direct member must receive one preparation");
    };
    let stamp = preparation.stamp();
    let entered = fixture_runtime
        .block_on(entries.recv())
        .expect("the executor holds the fixture's senders");
    match entered {
        Ok(ticket) => {
            assert_eq!(
                ticket.first().head().activation().output(),
                SessionFrame::new(32)
            );
            (ticket, stamp)
        }
        Err(receipt) => panic!("the executor rejected the entry: {receipt:?}"),
    }
}

/// The owner of a fresh member behind a fresh gate.
pub(crate) fn fresh_owner() -> MemberOwner {
    MemberOwner::new(BeatGridId::allocate().expect("fixture member identity"))
}

/// What a fixture reader answers to every read.
#[derive(Clone, Copy, Debug)]
pub(crate) enum ReaderMode {
    Silence,
    Eof,
    Failure,
    ShortThenEof,
}

/// A stereo resource at `rate` whose reader answers `mode`, calling
/// `on_read` first; `read_required` and `track` declare whether the test
/// must read it and ask its duration.
pub(crate) fn resource(
    mode: ReaderMode,
    rate: NonZeroU32,
    read_required: bool,
    track: bool,
    on_read: Option<Arc<dyn Fn() + Send + Sync>>,
) -> Box<PlayerResource> {
    let calls = AtomicUsize::new(0);
    let event_bus = AudioSessionMock::event_bus
        .each_call(matching!())
        .answers(&|mock| mock.make_ref(EventBus::new(1)));
    let spec = AudioReadMock::spec
        .each_call(matching!())
        .returns(AudioSpec::new(2, rate));
    let preload = AudioControlMock::preload
        .next_call(matching!())
        .returns(Ok(()));
    let reader = if read_required {
        let read = AudioReadMock::read_planar
            .each_call(matching!())
            .answers_arc(Arc::new(move |_, output| {
                if let Some(on_read) = &on_read {
                    on_read();
                }
                match mode {
                    ReaderMode::Silence => {
                        let frames = output[0].len();
                        for channel in output.iter_mut() {
                            channel.fill(0.0);
                        }
                        Ok(AudioReadOutcome::Frames {
                            count: NonZeroUsize::new(frames).expect("nonempty callback read"),
                            position: Duration::ZERO,
                            source_span: None,
                        })
                    }
                    ReaderMode::Eof => Ok(AudioReadOutcome::Eof {
                        position: Duration::ZERO,
                    }),
                    ReaderMode::Failure => Err(DecodeError::InvalidData {
                        detail: "fixture fault",
                    }),
                    ReaderMode::ShortThenEof if calls.fetch_add(1, Ordering::Relaxed) == 0 => {
                        let count = output[0].len().min(16);
                        for channel in output.iter_mut() {
                            channel[..count].fill(0.5);
                        }
                        Ok(AudioReadOutcome::Frames {
                            count: NonZeroUsize::new(count).expect("fixture read has space"),
                            position: Duration::ZERO,
                            source_span: None,
                        })
                    }
                    ReaderMode::ShortThenEof => Ok(AudioReadOutcome::Eof {
                        position: Duration::ZERO,
                    }),
                }
            }));
        if track {
            let duration = AudioSessionMock::duration
                .each_call(matching!())
                .returns(Some(Duration::from_secs(1)));
            Unimock::new((event_bus, duration, spec, preload, read))
        } else {
            Unimock::new((event_bus, spec, preload, read))
        }
    } else if track {
        let duration = AudioSessionMock::duration
            .each_call(matching!())
            .returns(Some(Duration::from_secs(1)));
        Unimock::new((event_bus, duration, spec, preload))
    } else {
        Unimock::new((event_bus, spec, preload))
    };
    let src: Arc<str> = Arc::from("fixture");
    let resource = Resource::from_reader(reader, Some(Arc::clone(&src)));
    Box::new(PlayerResource::new(resource, src, &pools()).expect("fixture resource fits pool"))
}
