use firewheel::{
    clock::InstantSamples,
    mask::{ConnectedMask, ConstantMask, SilenceMask},
    node::{ProcStore, StreamStatus},
};
use kithara_command::{
    Batch, ChannelConfig, Outcome, Port, Rejection, ScopedConfig, ScopedInbox, ScopedReceipt,
    ScopedSender, When, scoped_channel,
};
use kithara_effects::{
    GainDb,
    eq::{EqBandConfig, EqConfig, EqLayout},
};
use kithara_platform::{sync::Arc, time::Duration};
use kithara_signal::{AudioSpec, OutputContext, SessionEpoch, SessionFrame, TransportRevision};
use kithara_test_fixtures::integration_fixtures::constant_half;
use kithara_warp::RenderContext;
use num_traits::cast;

use super::*;
use crate::{
    CrossfadeCurve, CrossfadeSettings,
    bridge::{
        DeckEnds, DeckEqChange, DeckPart, DeckRefusal, Fade, FadeDir, Returned, scope_channels,
    },
    rt::{
        DeckMixerConfig,
        track::{PcmConsumer, PlayerResource},
    },
    test_pools::pools,
    worker::{
        PcmPacket,
        packet_tests::{PacketRing, chunk},
    },
};

const BLOCK: usize = 512;
const RATE: u32 = 44_100;
const LEVEL: f32 = 0.5;
const A: Slot = Slot::new(0);
const B: Slot = Slot::new(1);

#[kithara::test]
fn application_deadline_is_optional_but_explicit_geometry_is_enforced() {
    let shape = StreamShape::new(
        NonZeroU32::new(512).expect("fixture block"),
        NonZeroU32::new(48_000).expect("fixture rate"),
    );
    let quantum = NonZeroUsize::new(32).expect("fixture quantum");
    let (preload, ring) = shape
        .playback_buffers(quantum, None)
        .expect("unbounded deadline");
    assert_eq!((preload.get(), ring.get()), (16, 17));
    assert!(matches!(
        shape.playback_buffers(quantum, NonZeroUsize::new(448)),
        Err(BufferGeometryError::BudgetExceeded {
            required_frames: 575,
            max_block_frames: 512,
            render_quantum_frames: 32,
            budget_frames: 448,
        })
    ));
}

fn shape() -> StreamShape {
    StreamShape {
        sample_rate: NonZeroU32::new(RATE).expect("static sample rate"),
        max_block_frames: NonZeroU32::new(512).expect("static block size"),
    }
}

struct TestSessionInbox(ScopedInbox<DeckProtocol, DeckProtocol>);

impl SessionInbox for TestSessionInbox {
    fn scope(&mut self, id: ScopeId) -> Option<LevelInbox<'_, DeckProtocol>> {
        self.0.scope(id)
    }
}

struct TestMixer {
    mixer: DeckMixer<TestSessionInbox>,
    inbox: TestSessionInbox,
}

impl std::ops::Deref for TestMixer {
    type Target = DeckMixer<TestSessionInbox>;
    fn deref(&self) -> &Self::Target {
        &self.mixer
    }
}

struct TestEnds {
    ring: ScopedSender<DeckProtocol, DeckProtocol>,
    deck: DeckEnds,
}

impl std::ops::Deref for TestEnds {
    type Target = DeckEnds;
    fn deref(&self) -> &Self::Target {
        &self.deck
    }
}

impl std::ops::DerefMut for TestEnds {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.deck
    }
}

fn mixer() -> (TestMixer, TestEnds) {
    mixer_with_config(DeckMixerConfig::default(), 64)
}

fn mixer_with_config(config: DeckMixerConfig, capacity: usize) -> (TestMixer, TestEnds) {
    let (mut ring, inbox) = scoped_channel(
        ScopedConfig::builder()
            .scope(
                ChannelConfig::builder()
                    .targets(config.slots().get())
                    .capacity(NonZeroUsize::new(capacity).expect("capacity"))
                    .build(),
            )
            .build(),
    );
    let scope = ring.open(config.slots().get()).expect("deck scope");
    let (deck, inputs) = scope_channels(scope, config);
    let mixer = DeckMixer::new(inputs, shape(), &pools()).expect("mixer pools");
    (
        TestMixer {
            mixer,
            inbox: TestSessionInbox(inbox),
        },
        TestEnds { ring, deck },
    )
}

fn proc_info() -> ProcInfo {
    ProcInfo {
        sample_rate: NonZeroU32::new(RATE).expect("static sample rate"),
        frames: 512,
        in_silence_mask: SilenceMask::default(),
        out_silence_mask: SilenceMask::default(),
        in_constant_mask: ConstantMask::default(),
        out_constant_mask: ConstantMask::default(),
        in_connected_mask: ConnectedMask::default(),
        out_connected_mask: ConnectedMask::default(),
        total_cpu_seconds_recip: 1.0,
        process_to_playback_delay: None,
        did_just_unbypass: false,
        last_marker_instant: InstantSamples(0),
        sample_rate_recip: f64::from(RATE).recip(),
        clock_samples: InstantSamples(0),
        duration_since_stream_start: Duration::ZERO,
        stream_status: StreamStatus::empty(),
        dropped_frames: 0,
    }
}

#[kithara::test]
fn session_processors_read_the_same_host_context() {
    let mut store = ProcStore::with_capacity(1);
    super::super::install_render_context(&mut store)
        .expect("invariant: fixture installs one context slot");
    super::super::publish_render_context(
        &mut store,
        RenderContext::new_linear(
            OutputContext::new(
                SessionFrame::new(0)..SessionFrame::new(512),
                NonZeroU32::new(RATE).expect("static sample rate"),
                SessionEpoch::new(3),
                Some(TransportRevision::first()),
            )
            .expect("invariant: fixture output range is ordered"),
            None,
        )
        .expect("invariant: fixture context is valid"),
    )
    .expect("invariant: fixture context slot exists");
    let info = proc_info();
    let left = read_render_context(&store, &info).expect("session context");
    let right = read_render_context(&store, &info).expect("session context");

    assert!(std::ptr::eq(left, right));
    assert_eq!(left.output().session_epoch(), SessionEpoch::new(3));
    assert_eq!(
        left.output().transport_revision(),
        Some(TransportRevision::first())
    );
}

fn pcm(constant_half: &'static [u8], src: &str, seconds: f64) -> Box<PlayerResource> {
    let total = (seconds * f64::from(RATE)).floor() as usize;
    let frames = total.min(4096);
    let samples = constant_half
        .chunks_exact(4)
        .take(frames * 2)
        .map(|bytes| f32::from_le_bytes(bytes.try_into().expect("sample bytes")))
        .collect::<Vec<_>>();
    pcm_samples(src, seconds, &samples, frames == total)
}

fn pcm_samples(src: &str, seconds: f64, samples: &[f32], ended: bool) -> Box<PlayerResource> {
    let spec = AudioSpec::new(2, NonZeroU32::new(RATE).expect("sample rate"));
    let mut ring = PacketRing::new(spec, Duration::from_secs_f64(seconds), 2);
    ring.push(PcmPacket::Chunk(chunk(
        spec,
        SegmentId::FIRST,
        0,
        0,
        samples,
    )));
    if ended {
        let mut end = chunk(
            spec,
            SegmentId::FIRST,
            (samples.len() / 2) as u64,
            (samples.len() / 2) as u64,
            &[],
        );
        end.meta.end_of_track = true;
        ring.push(PcmPacket::Chunk(end));
    }
    Box::new(
        PlayerResource::new(
            PcmConsumer::new(ring.receiver.take().expect("receiver")),
            Arc::from(src),
            &pools(),
        )
        .expect("resource"),
    )
}

fn send(ring: &mut TestEnds, when: When<SessionFrame>, parts: Vec<DeckPart>) -> Seq {
    send_on(ring, when, &[], parts)
}

fn send_on(
    ring: &mut TestEnds,
    when: When<SessionFrame>,
    basis: &[(Slot, Option<Seq>)],
    commands: Vec<DeckPart>,
) -> Seq {
    let scope = ring.deck.scope;
    let seq = ring
        .ring
        .scope(scope)
        .expect("live scope")
        .send(
            when,
            Batch {
                basis: basis.to_vec(),
                commands,
            },
        )
        .expect("the deck ring has room");
    ring.ring.publish().expect("publish batch");
    seq
}

const fn at(frame: usize) -> When<SessionFrame> {
    When::At(frame_at(frame))
}

const fn frame_at(frame: usize) -> SessionFrame {
    SessionFrame::new(frame as i64)
}

/// Renders the block starting at session frame `start` and answers its left channel.
fn render(mixer: &mut TestMixer, start: usize) -> Vec<f32> {
    render_frames(mixer, start, BLOCK)
}

fn render_frames(mixer: &mut TestMixer, start: usize, frames: usize) -> Vec<f32> {
    let mut left = vec![0.0f32; frames];
    let mut right = vec![0.0f32; frames];
    let inputs: [&[f32]; 0] = [];
    let mut outputs = [&mut left[..], &mut right[..]];
    let mut buffers = ProcBuffers {
        inputs: &inputs,
        outputs: &mut outputs,
    };
    mixer.inbox.0.drain();
    let mut level = mixer.inbox.scope(mixer.mixer.scope).expect("live scope");
    let context = RenderContext::new_linear(
        OutputContext::new(
            frame_at(start)..frame_at(start + frames),
            shape().sample_rate,
            SessionEpoch::new(0),
            None,
        )
        .expect("output range"),
        None,
    )
    .expect("context");
    mixer.mixer.render_block_in(
        &mut level,
        Some(&context),
        frame_at(start),
        &mut buffers,
        frames,
    );
    mixer.mixer.publish(frame_at(start + frames));
    left
}

fn receipts(ends: &mut TestEnds) -> Vec<(Seq, Outcome<DeckProtocol>, Vec<DeckPart>)> {
    let mut receipts = Vec::new();
    while let Some(receipt) = ends.ring.receipt() {
        if let ScopedReceipt::Scope(scope, receipt) = receipt {
            assert_eq!(scope, ends.deck.scope);
            let seq = receipt.seq();
            let (outcome, batch): (Outcome<DeckProtocol>, Batch<DeckProtocol>) = receipt.into();
            receipts.push((seq, outcome, batch.commands));
        }
    }
    receipts
}

fn outcome_of(
    receipts: &[(Seq, Outcome<DeckProtocol>, Vec<DeckPart>)],
    seq: Seq,
) -> &Outcome<DeckProtocol> {
    receipts
        .iter()
        .find_map(|(answered, outcome, _)| (*answered == seq).then_some(outcome))
        .expect("the batch is answered")
}

fn assert_applied_at(outcome: &Outcome<DeckProtocol>, frame: usize) {
    assert!(
        matches!(outcome, Outcome::Applied { at, data: () } if *at == frame_at(frame)),
        "expected applied at {frame}, got {outcome:?}"
    );
}

fn eq_layout(gains: &[GainDb]) -> Box<EqLayout> {
    let bands: Vec<_> = gains
        .iter()
        .map(|gain| EqBandConfig::builder().gain_db(*gain).build())
        .collect();
    Box::new(
        EqLayout::new(
            &EqConfig::builder(pools()).build(),
            &bands,
            NonZeroU32::new(RATE).expect("static sample rate"),
        )
        .expect("an EQ layout fits the test pool budget"),
    )
}

fn start_dc(mixer: &mut TestMixer, ends: &mut TestEnds, slots: &[Slot]) {
    let mut parts = Vec::new();
    for slot in slots {
        parts.push(DeckPart::Attach {
            slot: *slot,
            pcm: pcm_samples("dc", 1.0, &vec![1.0; 8192], false),
            segment: SegmentId::FIRST,
        });
        parts.push(DeckPart::Start {
            slot: *slot,
            fade: Fade::Crossfade(
                CrossfadeSettings::new(0.0, CrossfadeCurve::Linear, 1.0, 0.5)
                    .expect("instant start"),
            ),
        });
    }
    send(ends, When::Next, parts);
    render(mixer, 0);
    assert_eq!(receipts(ends).len(), 1);
}

fn assert_stopped_once(
    replies: &[(Seq, Outcome<DeckProtocol>, Vec<DeckPart>)],
    seq: Seq,
    committed_at: usize,
    slot: Slot,
    interrupt_at: usize,
) {
    let matches = replies
        .iter()
        .filter(|(answered, ..)| *answered == seq)
        .collect::<Vec<_>>();
    assert_eq!(
        matches.len(),
        1,
        "every committed batch has exactly one verdict"
    );
    let (_, outcome, parts) = matches[0];
    assert_applied_at(outcome, committed_at);
    let marks = parts
        .iter()
        .filter_map(|part| match part {
            DeckPart::Returned(Returned::Stopped {
                slot: stopped,
                resume,
            }) if *stopped == slot => Some(*resume),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        marks,
        [crate::bridge::SlotMark {
            session: frame_at(interrupt_at),
            lane: crate::LaneFrame {
                segment: SegmentId::FIRST,
                frame: interrupt_at as u64
            },
            position: AudioSpec::new(2, shape().sample_rate)
                .duration_for(interrupt_at as u64)
                .expect("position"),
        }]
    );
}

#[kithara::test]
#[case::stop_then_start(false)]
#[case::stop_then_replace(true)]
fn an_interrupted_stop_still_answers_its_batch(#[case] replace: bool) {
    let (mut mixer, mut ends) = mixer();
    start_dc(&mut mixer, &mut ends, &[A]);
    let settings =
        CrossfadeSettings::new(2.0, CrossfadeCurve::Linear, 1.0, 0.5).expect("long ramp");
    let stop = send(
        &mut ends,
        at(BLOCK),
        vec![DeckPart::Stop {
            slot: A,
            fade: Fade::Crossfade(settings),
        }],
    );
    let interrupted_at = BLOCK + 32;
    let interrupt = send(
        &mut ends,
        at(interrupted_at),
        vec![if replace {
            DeckPart::Replace {
                slot: A,
                pcm: pcm_samples("replacement", 1.0, &vec![1.0; 8192], false),
                segment: SegmentId::FIRST,
            }
        } else {
            DeckPart::Start {
                slot: A,
                fade: Fade::Declick,
            }
        }],
    );
    render(&mut mixer, BLOCK);
    let replies = receipts(&mut ends);
    assert_stopped_once(&replies, stop, BLOCK, A, interrupted_at);
    assert_eq!(
        replies.iter().filter(|(seq, ..)| *seq == interrupt).count(),
        1
    );
    assert_applied_at(outcome_of(&replies, interrupt), interrupted_at);
    render(&mut mixer, 2 * BLOCK);
    assert!(
        receipts(&mut ends).is_empty(),
        "neither batch is answered twice"
    );
}

#[kithara::test]
fn a_stop_followed_by_start_in_one_batch_returns_its_interrupt_mark() {
    let (mut mixer, mut ends) = mixer();
    start_dc(&mut mixer, &mut ends, &[A]);
    let seq = send(
        &mut ends,
        at(BLOCK),
        vec![
            DeckPart::Stop {
                slot: A,
                fade: Fade::Declick,
            },
            DeckPart::Start {
                slot: A,
                fade: Fade::Declick,
            },
        ],
    );
    render(&mut mixer, BLOCK);
    assert_stopped_once(&receipts(&mut ends), seq, BLOCK, A, BLOCK);
    render(&mut mixer, 2 * BLOCK);
    assert!(receipts(&mut ends).is_empty(), "one verdict per batch");
}

#[kithara::test]
fn a_multislot_stop_completes_when_one_slot_is_interrupted() {
    let (mut mixer, mut ends) = mixer();
    start_dc(&mut mixer, &mut ends, &[A, B]);
    let seq = send(
        &mut ends,
        at(BLOCK),
        vec![
            DeckPart::Stop {
                slot: A,
                fade: Fade::Declick,
            },
            DeckPart::Stop {
                slot: B,
                fade: Fade::Declick,
            },
        ],
    );
    send(
        &mut ends,
        at(BLOCK + 32),
        vec![DeckPart::Start {
            slot: A,
            fade: Fade::Declick,
        }],
    );
    render(&mut mixer, BLOCK);
    let replies = receipts(&mut ends);
    assert_stopped_once(&replies, seq, BLOCK, A, BLOCK + 32);
    assert_stopped_once(&replies, seq, BLOCK, B, BLOCK + 221);
    render(&mut mixer, 2 * BLOCK);
    assert!(receipts(&mut ends).is_empty());
}

#[kithara::test]
fn interrupted_stops_release_credit_beyond_the_deck_capacity() {
    const CAPACITY: usize = 4;
    let (mut mixer, mut ends) = mixer_with_config(DeckMixerConfig::default(), CAPACITY);
    start_dc(&mut mixer, &mut ends, &[A]);
    let settings =
        CrossfadeSettings::new(2.0, CrossfadeCurve::Linear, 1.0, 0.5).expect("long ramp");
    for iteration in 0..2 * CAPACITY {
        let frame = BLOCK + iteration * 32;
        let scope = ends.deck.scope;
        for (at, command) in [
            (
                frame,
                DeckPart::Stop {
                    slot: A,
                    fade: Fade::Crossfade(settings),
                },
            ),
            (
                frame + 16,
                DeckPart::Start {
                    slot: A,
                    fade: Fade::Declick,
                },
            ),
        ] {
            let sent = ends.ring.scope(scope).expect("scope").send(
                self::at(at),
                Batch {
                    basis: Vec::new(),
                    commands: vec![command],
                },
            );
            assert!(
                sent.is_ok(),
                "interrupt {iteration} at {at} must not leak credit or return Full: {sent:?}"
            );
        }
        ends.ring.publish().expect("publish");
        render_frames(&mut mixer, frame, 32);
        drop(receipts(&mut ends));
    }
}

#[kithara::test]
#[case::samples(false)]
#[case::mark(true)]
fn adopt_never_mixes_a_blocked_older_packet_or_publishes_its_mark(#[case] check_mark: bool) {
    let config = DeckMixerConfig::builder()
        .recycle_per_block(NonZeroUsize::new(1).expect("one recycle per block"))
        .build();
    let (mut mixer, mut ends) = mixer_with_config(config, 8);
    let spec = AudioSpec::new(2, shape().sample_rate);
    let old = SegmentId::FIRST;
    let current = old.next();
    let mut packets = PacketRing::new(spec, Duration::from_secs(1), 2);
    let mut receiver = packets.receiver.take().expect("receiver");
    for _ in 0..2 {
        receiver
            .recycle(PcmPacket::Chunk(chunk(spec, old, 0, 0, &[1.0; 2])))
            .expect("fill reverse ring");
    }
    assert!(
        receiver
            .recycle(PcmPacket::Chunk(chunk(spec, old, 0, 0, &[1.0; 2])))
            .is_err()
    );
    packets.push(PcmPacket::Chunk(chunk(spec, old, 0, 0, &[1.0; 32])));
    let resource = Box::new(
        PlayerResource::new(PcmConsumer::new(receiver), Arc::from("segments"), &pools())
            .expect("resource"),
    );
    let instant = Fade::Crossfade(
        CrossfadeSettings::new(0.0, CrossfadeCurve::Linear, 1.0, 0.5).expect("instant envelope"),
    );
    send(
        &mut ends,
        at(0),
        vec![
            DeckPart::Attach {
                slot: A,
                pcm: resource,
                segment: old,
            },
            DeckPart::Start {
                slot: A,
                fade: instant,
            },
        ],
    );
    assert_eq!(render_frames(&mut mixer, 0, 1), [1.0]);
    drop(receipts(&mut ends));
    packets.push(PcmPacket::Chunk(chunk(
        spec, current, 100, 1000, &[2.0; 16],
    )));
    packets.push(PcmPacket::Chunk(chunk(
        spec, current, 108, 1008, &[2.0; 16],
    )));
    let adopt = send(
        &mut ends,
        at(1),
        vec![
            DeckPart::Stop {
                slot: A,
                fade: instant,
            },
            DeckPart::Adopt {
                slot: A,
                segment: current,
            },
            DeckPart::Start {
                slot: A,
                fade: instant,
            },
        ],
    );
    let mixed = render_frames(&mut mixer, 1, 4);
    assert_applied_at(outcome_of(&receipts(&mut ends), adopt), 1);
    if check_mark {
        assert_eq!(
            mixer.track(A).expect("track").mark(frame_at(5)),
            None,
            "the old held packet cannot map the adopted slot"
        );
    } else {
        assert_eq!(mixed, [0.0; 4], "no old packet reaches the mix after Adopt");
    }
    while packets.returned().is_some() {}
    assert_eq!(render_frames(&mut mixer, 5, 4), [2.0; 4]);
    assert_eq!(
        mixer.track(A).expect("track").mark(frame_at(9)),
        Some(crate::bridge::SlotMark {
            session: frame_at(9),
            lane: crate::LaneFrame {
                segment: current,
                frame: 104
            },
            position: spec.duration_for(1004).expect("new source position"),
        })
    );
}

#[kithara::test]
#[case::detach(false)]
#[case::replace(true)]
fn removing_a_slot_while_its_tail_sounds_preserves_gain_continuity(#[case] replace: bool) {
    let (mut mixer, mut ends) = mixer();
    start_dc(&mut mixer, &mut ends, &[A]);
    send(
        &mut ends,
        at(BLOCK),
        vec![DeckPart::Replace {
            slot: A,
            pcm: pcm_samples("second dc", 1.0, &vec![1.0; 8192], false),
            segment: SegmentId::FIRST,
        }],
    );
    const CUT: usize = 100;
    send(
        &mut ends,
        at(BLOCK + CUT),
        vec![if replace {
            DeckPart::Replace {
                slot: A,
                pcm: pcm_samples("silence", 1.0, &vec![0.0; 8192], false),
                segment: SegmentId::FIRST,
            }
        } else {
            DeckPart::Detach { slot: A }
        }],
    );
    let left = render(&mut mixer, BLOCK);
    assert!(
        left[CUT - 1] > 1.1,
        "both the existing tail and the new DC are sounding before removal"
    );
    let step = left[CUT - 1..CUT + 8]
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .fold(0.0_f32, f32::max);
    assert!(
        step < 0.02,
        "a sounding tail must not hard-cut its outgoing full-scale DC: step {step}"
    );
}

/// A slot started on a frame inside a block is silent before that frame and sounds after it.
#[kithara::test(tokio)]
async fn a_started_slot_sounds_from_its_frame(constant_half: &'static [u8]) {
    const START: usize = 100;
    let (mut mixer, mut ends) = mixer();
    send(
        &mut ends,
        When::Next,
        vec![DeckPart::Attach {
            slot: A,
            pcm: pcm(constant_half, "a", 60.0),
            segment: SegmentId::FIRST,
        }],
    );
    let start = send_on(
        &mut ends,
        at(START),
        &[(A, None)],
        vec![DeckPart::Start {
            slot: A,
            fade: Fade::Declick,
        }],
    );

    let left = render(&mut mixer, 0);

    assert!(left[..START].iter().all(|sample| *sample == 0.0));
    assert!(left[BLOCK - 1] > 0.0, "the slot sounds after its start");
    assert_applied_at(outcome_of(&receipts(&mut ends), start), START);
}

/// A slot that enters with a crossfade sounds the incoming gain of `gains` frame by frame from
/// the frame it started on.
#[kithara::test(tokio)]
async fn a_crossfade_start_follows_the_incoming_gains(constant_half: &'static [u8]) {
    let settings = CrossfadeSettings::new(0.016, CrossfadeCurve::EqualPower, 0.7, 0.3)
        .expect("valid settings");
    let (mut mixer, mut ends) = mixer();
    send(
        &mut ends,
        When::Next,
        vec![
            DeckPart::Attach {
                slot: A,
                pcm: pcm(constant_half, "a", 60.0),
                segment: SegmentId::FIRST,
            },
            DeckPart::Start {
                slot: A,
                fade: Fade::Crossfade(settings),
            },
        ],
    );

    let left = render(&mut mixer, 0);

    let frames = (0.016_f32 * 44_100.0).round();
    for (frame, sample) in left.iter().enumerate() {
        let progress = cast::<usize, f32>(frame).unwrap_or(f32::MAX) / (frames - 1.0);
        let (_, into) = settings.gains(progress);
        assert!(
            (sample - LEVEL * into).abs() < 1e-6,
            "frame {frame}: {sample} against {}",
            LEVEL * into
        );
    }
}

/// A fade down to silence stops its slot and reports the frame it went silent on.
#[kithara::test(tokio)]
async fn a_fade_to_silence_stops_the_slot_and_reports_faded(constant_half: &'static [u8]) {
    const FADE_AT: usize = BLOCK + 10;
    let settings =
        CrossfadeSettings::new(0.004, CrossfadeCurve::Linear, 1.0, 0.5).expect("valid settings");
    let (mut mixer, mut ends) = mixer();
    send(
        &mut ends,
        When::Next,
        vec![
            DeckPart::Attach {
                slot: A,
                pcm: pcm(constant_half, "a", 60.0),
                segment: SegmentId::FIRST,
            },
            DeckPart::Start {
                slot: A,
                fade: Fade::Declick,
            },
        ],
    );
    render(&mut mixer, 0);
    send(
        &mut ends,
        at(FADE_AT),
        vec![DeckPart::Fade {
            slot: A,
            settings,
            dir: FadeDir::Out,
        }],
    );

    render(&mut mixer, BLOCK);

    let frames = 176;
    assert_eq!(
        ends.events.drain().collect::<Vec<_>>(),
        [DeckEvent::Faded {
            slot: A,
            at: frame_at(FADE_AT + frames),
        }]
    );
    assert_eq!(
        mixer.track(A).map(PlayerTrack::state),
        Some(SlotState::Stopped)
    );
}

/// A replace on a frame plays the old consumer's next frames out of the slot's tail, ramped down
/// to silence under the new consumer that starts on the same frame, and returns the old one.
#[kithara::test(tokio)]
async fn a_replace_plays_the_old_consumer_out_of_the_tail(constant_half: &'static [u8]) {
    const REPLACE_AT: usize = BLOCK + 100;
    let (mut mixer, mut ends) = mixer();
    send(
        &mut ends,
        When::Next,
        vec![
            DeckPart::Attach {
                slot: A,
                pcm: pcm(constant_half, "old", 60.0),
                segment: SegmentId::FIRST,
            },
            DeckPart::Start {
                slot: A,
                fade: Fade::Declick,
            },
        ],
    );
    render(&mut mixer, 0);
    let replace = send_on(
        &mut ends,
        at(REPLACE_AT),
        &[(A, None)],
        vec![DeckPart::Replace {
            slot: A,
            pcm: pcm(constant_half, "new", 60.0),
            segment: SegmentId::FIRST,
        }],
    );

    let left = render(&mut mixer, BLOCK);

    let offset = REPLACE_AT - BLOCK;
    assert!((left[offset - 1] - LEVEL).abs() < 1e-6);
    assert!(
        (left[offset] - 2.0 * LEVEL).abs() < 1e-6,
        "the tail starts at full gain under the new consumer: {}",
        left[offset]
    );
    assert!(
        left[offset..].windows(2).all(|pair| pair[1] <= pair[0]),
        "the tail ramps down"
    );
    let receipts = receipts(&mut ends);
    let (_, outcome, parts) = receipts
        .iter()
        .find(|(seq, ..)| *seq == replace)
        .expect("the replace is answered");
    assert_applied_at(outcome, REPLACE_AT);
    assert!(
        matches!(parts.as_slice(), [DeckPart::Returned(Returned::Pcm { slot, pcm: old })] if *slot == A && &**old.src() == "old"),
        "the old consumer comes back: {parts:?}"
    );
}

/// A chain starts its slot on the frame after the other slot's last, sample for sample, and
/// applies on that frame.
#[kithara::test(tokio)]
async fn a_chain_starts_its_slot_on_the_frame_after_the_end(constant_half: &'static [u8]) {
    let (mut mixer, mut ends) = mixer();
    send(
        &mut ends,
        When::Next,
        vec![
            DeckPart::Attach {
                slot: A,
                pcm: pcm(constant_half, "ending", 0.005),
                segment: SegmentId::FIRST,
            },
            DeckPart::Attach {
                slot: B,
                pcm: pcm(constant_half, "next", 60.0),
                segment: SegmentId::FIRST,
            },
            DeckPart::Start {
                slot: A,
                fade: Fade::Declick,
            },
        ],
    );
    let chain = send_on(
        &mut ends,
        When::Deferred,
        &[(B, None)],
        vec![DeckPart::Chain { from: A, to: B }],
    );

    let left = render(&mut mixer, 0);

    let ended = ends
        .events
        .drain()
        .find_map(|event| match event {
            DeckEvent::Ended { slot, at } if slot == A => Some(at),
            _ => None,
        })
        .expect("the first slot ends inside the block");
    let end = usize::try_from(i64::from(ended)).expect("the end is in the block");
    assert!(
        left.iter().all(|sample| (sample - LEVEL).abs() < 1e-6),
        "no frame is lost at the seam"
    );
    assert_applied_at(outcome_of(&receipts(&mut ends), chain), end);
    assert_eq!(
        mixer.track(B).map(PlayerTrack::state),
        Some(SlotState::Playing)
    );
}

/// A chain whose slot another batch shifted while it waited comes back stale.
#[kithara::test(tokio)]
async fn a_chain_whose_slot_shifted_comes_back_stale(constant_half: &'static [u8]) {
    let (mut mixer, mut ends) = mixer();
    send(
        &mut ends,
        When::Next,
        vec![
            DeckPart::Attach {
                slot: A,
                pcm: pcm(constant_half, "ending", 0.005),
                segment: SegmentId::FIRST,
            },
            DeckPart::Attach {
                slot: B,
                pcm: pcm(constant_half, "next", 60.0),
                segment: SegmentId::FIRST,
            },
            DeckPart::Start {
                slot: A,
                fade: Fade::Declick,
            },
        ],
    );
    let chain = send_on(
        &mut ends,
        When::Deferred,
        &[(B, None)],
        vec![DeckPart::Chain { from: A, to: B }],
    );
    send_on(
        &mut ends,
        at(10),
        &[(B, None)],
        vec![DeckPart::Adopt {
            slot: B,
            segment: SegmentId::FIRST.next(),
        }],
    );

    render(&mut mixer, 0);

    assert!(matches!(
        outcome_of(&receipts(&mut ends), chain),
        Outcome::Rejected(Rejection::Stale)
    ));
    assert_eq!(
        mixer.track(B).map(PlayerTrack::state),
        Some(SlotState::Stopped)
    );
}

/// Detaching a chained slot refuses the chain behind it: attached again, the slot waits for its
/// own start instead of being started by the chain.
#[kithara::test(tokio)]
async fn a_detached_slot_refuses_the_chain_behind_it(constant_half: &'static [u8]) {
    let (mut mixer, mut ends) = mixer();
    send(
        &mut ends,
        When::Next,
        vec![
            DeckPart::Attach {
                slot: A,
                pcm: pcm(constant_half, "ending", 0.005),
                segment: SegmentId::FIRST,
            },
            DeckPart::Attach {
                slot: B,
                pcm: pcm(constant_half, "next", 60.0),
                segment: SegmentId::FIRST,
            },
        ],
    );
    let chain = send(
        &mut ends,
        When::Deferred,
        vec![DeckPart::Chain { from: A, to: B }],
    );
    send(&mut ends, When::Next, vec![DeckPart::Detach { slot: B }]);
    send(
        &mut ends,
        When::Next,
        vec![
            DeckPart::Attach {
                slot: B,
                pcm: pcm(constant_half, "next", 60.0),
                segment: SegmentId::FIRST,
            },
            DeckPart::Start {
                slot: A,
                fade: Fade::Declick,
            },
        ],
    );

    render(&mut mixer, 0);
    render(&mut mixer, BLOCK);

    assert!(matches!(
        outcome_of(&receipts(&mut ends), chain),
        Outcome::Rejected(Rejection::Refused(DeckRefusal::Empty { slot: B }))
    ));
    assert_eq!(
        mixer.track(B).map(PlayerTrack::state),
        Some(SlotState::Stopped)
    );
}

/// Attaching to a slot that holds a track refuses the whole batch.
#[kithara::test(tokio)]
async fn an_attach_to_an_occupied_slot_is_refused(constant_half: &'static [u8]) {
    let (mut mixer, mut ends) = mixer();
    send(
        &mut ends,
        When::Next,
        vec![DeckPart::Attach {
            slot: A,
            pcm: pcm(constant_half, "held", 60.0),
            segment: SegmentId::FIRST,
        }],
    );
    let second = send(
        &mut ends,
        When::Next,
        vec![DeckPart::Attach {
            slot: A,
            pcm: pcm(constant_half, "refused", 60.0),
            segment: SegmentId::FIRST,
        }],
    );

    render(&mut mixer, 0);

    let receipts = receipts(&mut ends);
    assert!(matches!(
        outcome_of(&receipts, second),
        Outcome::Rejected(Rejection::Refused(DeckRefusal::Occupied { slot: A }))
    ));
    assert_eq!(mixer.track(A).map(|track| &**track.src()), Some("held"));
}

/// A band cut sent to a playing deck rides the deck's own ring: the block after it renders the
/// deck lowered by the cut.
#[kithara::test(tokio)]
async fn a_band_cut_lowers_the_deck_output_in_the_next_block(constant_half: &'static [u8]) {
    const CUT_DB: f32 = -12.0;
    let (mut mixer, mut ends) = mixer();
    send(
        &mut ends,
        When::Next,
        vec![
            DeckPart::Attach {
                slot: A,
                pcm: pcm(constant_half, "eq", 60.0),
                segment: SegmentId::FIRST,
            },
            DeckPart::Eq(DeckEqChange::Layout(eq_layout(&[GainDb::DEFAULT]))),
            DeckPart::Start {
                slot: A,
                fade: Fade::Declick,
            },
        ],
    );
    render(&mut mixer, 0);
    let open = render(&mut mixer, BLOCK)[BLOCK - 1];

    send(
        &mut ends,
        When::Next,
        vec![DeckPart::Eq(DeckEqChange::Gain {
            band: 0,
            gain: GainDb::from(CUT_DB),
        })],
    );
    let cut = render(&mut mixer, 2 * BLOCK)[BLOCK - 1];

    let expected = open * 10f32.powf(CUT_DB / 20.0);
    assert!(open > 0.1, "the deck sounds before the cut: {open}");
    assert!(
        (cut - expected).abs() < 1e-3,
        "cut {cut}, expected {expected} from {open}"
    );
}

/// Events the owner does not drain are dropped once the ring is full and counted in the
/// snapshot, instead of blocking the audio thread.
#[kithara::test(tokio)]
async fn a_full_event_ring_counts_its_overflows(constant_half: &'static [u8]) {
    let (mut mixer, mut ends) = mixer();
    let slots = DeckMixerConfig::default().slots().get();
    let capacity = slots * 16;
    for _ in 0..=capacity {
        send(
            &mut ends,
            When::Next,
            vec![
                DeckPart::Attach {
                    slot: A,
                    pcm: pcm(constant_half, "short", 0.001),
                    segment: SegmentId::FIRST,
                },
                DeckPart::Start {
                    slot: A,
                    fade: Fade::Declick,
                },
            ],
        );
        render(&mut mixer, 0);
        send(&mut ends, When::Next, vec![DeckPart::Detach { slot: A }]);
        render(&mut mixer, 0);
        drop(receipts(&mut ends));
    }

    let snapshot = ends.snapshot.read();
    assert_eq!(snapshot.metrics.event_overflows(), 1);
    assert_eq!(ends.events.drain().count(), capacity);
}
