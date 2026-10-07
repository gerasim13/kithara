use firewheel::{
    clock::InstantSamples,
    mask::{ConnectedMask, ConstantMask, SilenceMask},
    node::{ProcStore, StreamStatus},
};
use kithara_audio::mock::TestPcmReader;
use kithara_command::{Batch, Outcome, Rejection, Sender, When};
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
        DeckApplied, DeckEnds, DeckEqChange, DeckPart, DeckRefusal, Fade, FadeDir, Released,
        mixer_channels,
    },
    rt::{
        DeckMixerConfig,
        track::{PcmConsumer, PlayerResource},
    },
    test_pools::pools,
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

fn mixer() -> (DeckMixer, DeckEnds) {
    let (ends, inputs) = mixer_channels(DeckMixerConfig::default());
    (DeckMixer::new(inputs, shape(), &pools()), ends)
}

fn session_mixer() -> DeckMixer {
    let (_ends, inputs) = mixer_channels(DeckMixerConfig::default());
    DeckMixer::with_context_requirement(inputs, shape(), &pools(), ContextRequirement::Session)
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
    let left = session_mixer();
    let right = session_mixer();
    let left = left
        .render_context(&store, &info)
        .expect("session context")
        .expect("required context");
    let right = right
        .render_context(&store, &info)
        .expect("session context")
        .expect("required context");

    assert!(std::ptr::eq(left, right));
    assert_eq!(left.output().session_epoch(), SessionEpoch::new(3));
    assert_eq!(
        left.output().transport_revision(),
        Some(TransportRevision::first())
    );
}

fn pcm(constant_half: &'static [u8], src: &str, seconds: f64) -> Box<PlayerResource> {
    let spec = AudioSpec::new(2, NonZeroU32::new(RATE).expect("static sample rate"));
    let reader = TestPcmReader::with_pcm(spec, seconds, constant_half);
    Box::new(
        PlayerResource::new(PcmConsumer::new(Box::new(reader)), Arc::from(src), &pools())
            .expect("player resource fits the test pool budget"),
    )
}

fn send(ring: &mut Sender<DeckProtocol>, when: When<SessionFrame>, parts: Vec<DeckPart>) -> Seq {
    send_on(ring, when, &[], parts)
}

fn send_on(
    ring: &mut Sender<DeckProtocol>,
    when: When<SessionFrame>,
    basis: &[(Slot, Option<Seq>)],
    commands: Vec<DeckPart>,
) -> Seq {
    ring.send(
        when,
        Batch {
            basis: basis.to_vec(),
            commands,
        },
    )
    .expect("the deck ring has room")
}

const fn at(frame: usize) -> When<SessionFrame> {
    When::At(frame_at(frame))
}

const fn frame_at(frame: usize) -> SessionFrame {
    SessionFrame::new(frame as i64)
}

/// Renders the block starting at session frame `start` and answers its left channel.
fn render(mixer: &mut DeckMixer, start: usize) -> Vec<f32> {
    let mut left = vec![0.0f32; BLOCK];
    let mut right = vec![0.0f32; BLOCK];
    let inputs: [&[f32]; 0] = [];
    let mut outputs = [&mut left[..], &mut right[..]];
    let mut buffers = ProcBuffers {
        inputs: &inputs,
        outputs: &mut outputs,
    };
    mixer.render_block(frame_at(start), &mut buffers, BLOCK);
    left
}

/// Every receipt the ring holds, as its number, outcome and returned parts.
fn receipts(ends: &mut DeckEnds) -> Vec<(Seq, Outcome<DeckProtocol>, Vec<DeckPart>)> {
    ends.ring
        .receipts()
        .map(|receipt| {
            let seq = receipt.seq();
            let (outcome, batch): (Outcome<DeckProtocol>, Batch<DeckProtocol>) = receipt.into();
            (seq, outcome, batch.commands)
        })
        .collect()
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

fn applied_at(frame: usize) -> Outcome<DeckProtocol> {
    Outcome::Applied {
        at: frame_at(frame),
        data: DeckApplied::default(),
    }
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

/// A slot started on a frame inside a block is silent before that frame and sounds after it.
#[kithara::test(tokio)]
async fn a_started_slot_sounds_from_its_frame(constant_half: &'static [u8]) {
    const START: usize = 100;
    let (mut mixer, mut ends) = mixer();
    send(
        &mut ends.ring,
        When::Next,
        vec![DeckPart::Attach {
            slot: A,
            pcm: pcm(constant_half, "a", 60.0),
        }],
    );
    let start = send_on(
        &mut ends.ring,
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
    assert_eq!(outcome_of(&receipts(&mut ends), start), &applied_at(START));
}

/// A slot that enters with a crossfade sounds the incoming gain of `gains` frame by frame from
/// the frame it started on.
#[kithara::test(tokio)]
async fn a_crossfade_start_follows_the_incoming_gains(constant_half: &'static [u8]) {
    let settings = CrossfadeSettings::new(0.016, CrossfadeCurve::EqualPower, 0.7, 0.3)
        .expect("valid settings");
    let (mut mixer, mut ends) = mixer();
    send(
        &mut ends.ring,
        When::Next,
        vec![
            DeckPart::Attach {
                slot: A,
                pcm: pcm(constant_half, "a", 60.0),
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
        &mut ends.ring,
        When::Next,
        vec![
            DeckPart::Attach {
                slot: A,
                pcm: pcm(constant_half, "a", 60.0),
            },
            DeckPart::Start {
                slot: A,
                fade: Fade::Declick,
            },
        ],
    );
    render(&mut mixer, 0);
    send(
        &mut ends.ring,
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
        &mut ends.ring,
        When::Next,
        vec![
            DeckPart::Attach {
                slot: A,
                pcm: pcm(constant_half, "old", 60.0),
            },
            DeckPart::Start {
                slot: A,
                fade: Fade::Declick,
            },
        ],
    );
    render(&mut mixer, 0);
    let replace = send_on(
        &mut ends.ring,
        at(REPLACE_AT),
        &[(A, None)],
        vec![DeckPart::Replace {
            slot: A,
            pcm: pcm(constant_half, "new", 60.0),
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
    assert_eq!(outcome, &applied_at(REPLACE_AT));
    assert!(
        matches!(parts.as_slice(), [DeckPart::Released(Released::Pcm { slot, pcm: old })] if *slot == A && &**old.src() == "old"),
        "the old consumer comes back: {parts:?}"
    );
}

/// A chain starts its slot on the frame after the other slot's last, sample for sample, and
/// applies on that frame.
#[kithara::test(tokio)]
async fn a_chain_starts_its_slot_on_the_frame_after_the_end(constant_half: &'static [u8]) {
    let (mut mixer, mut ends) = mixer();
    send(
        &mut ends.ring,
        When::Next,
        vec![
            DeckPart::Attach {
                slot: A,
                pcm: pcm(constant_half, "ending", 0.005),
            },
            DeckPart::Attach {
                slot: B,
                pcm: pcm(constant_half, "next", 60.0),
            },
            DeckPart::Start {
                slot: A,
                fade: Fade::Declick,
            },
        ],
    );
    let chain = send_on(
        &mut ends.ring,
        When::Next,
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
    assert_eq!(outcome_of(&receipts(&mut ends), chain), &applied_at(end));
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
        &mut ends.ring,
        When::Next,
        vec![
            DeckPart::Attach {
                slot: A,
                pcm: pcm(constant_half, "ending", 0.005),
            },
            DeckPart::Attach {
                slot: B,
                pcm: pcm(constant_half, "next", 60.0),
            },
            DeckPart::Start {
                slot: A,
                fade: Fade::Declick,
            },
        ],
    );
    let chain = send_on(
        &mut ends.ring,
        When::Next,
        &[(B, None)],
        vec![DeckPart::Chain { from: A, to: B }],
    );
    send_on(
        &mut ends.ring,
        at(10),
        &[(B, None)],
        vec![DeckPart::Seek {
            slot: B,
            seconds: 1.0,
            seek_epoch: 1,
        }],
    );

    render(&mut mixer, 0);

    assert_eq!(
        outcome_of(&receipts(&mut ends), chain),
        &Outcome::Rejected(Rejection::Stale)
    );
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
        &mut ends.ring,
        When::Next,
        vec![
            DeckPart::Attach {
                slot: A,
                pcm: pcm(constant_half, "ending", 0.005),
            },
            DeckPart::Attach {
                slot: B,
                pcm: pcm(constant_half, "next", 60.0),
            },
        ],
    );
    let chain = send(
        &mut ends.ring,
        When::Next,
        vec![DeckPart::Chain { from: A, to: B }],
    );
    send(&mut ends.ring, When::Next, vec![DeckPart::Detach { slot: B }]);
    send(
        &mut ends.ring,
        When::Next,
        vec![
            DeckPart::Attach {
                slot: B,
                pcm: pcm(constant_half, "next", 60.0),
            },
            DeckPart::Start {
                slot: A,
                fade: Fade::Declick,
            },
        ],
    );

    render(&mut mixer, 0);
    render(&mut mixer, BLOCK);

    assert_eq!(
        outcome_of(&receipts(&mut ends), chain),
        &Outcome::Rejected(Rejection::Refused(DeckRefusal::Empty { slot: B }))
    );
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
        &mut ends.ring,
        When::Next,
        vec![DeckPart::Attach {
            slot: A,
            pcm: pcm(constant_half, "held", 60.0),
        }],
    );
    let second = send(
        &mut ends.ring,
        When::Next,
        vec![DeckPart::Attach {
            slot: A,
            pcm: pcm(constant_half, "refused", 60.0),
        }],
    );

    render(&mut mixer, 0);

    let receipts = receipts(&mut ends);
    assert_eq!(
        outcome_of(&receipts, second),
        &Outcome::Rejected(Rejection::Refused(DeckRefusal::Occupied { slot: A }))
    );
    assert_eq!(
        mixer.track(A).map(|track| &**track.src()),
        Some("held")
    );
}

/// A band cut sent to a playing deck rides the deck's own ring: the block after it renders the
/// deck lowered by the cut.
#[kithara::test(tokio)]
async fn a_band_cut_lowers_the_deck_output_in_the_next_block(constant_half: &'static [u8]) {
    const CUT_DB: f32 = -12.0;
    let (mut mixer, mut ends) = mixer();
    send(
        &mut ends.ring,
        When::Next,
        vec![
            DeckPart::Attach {
                slot: A,
                pcm: pcm(constant_half, "eq", 60.0),
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
        &mut ends.ring,
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
            &mut ends.ring,
            When::Next,
            vec![
                DeckPart::Attach {
                    slot: A,
                    pcm: pcm(constant_half, "short", 0.001),
                },
                DeckPart::Start {
                    slot: A,
                    fade: Fade::Declick,
                },
            ],
        );
        render(&mut mixer, 0);
        send(&mut ends.ring, When::Next, vec![DeckPart::Detach { slot: A }]);
        render(&mut mixer, 0);
        drop(receipts(&mut ends));
    }

    let snapshot = ends.snapshot.read();
    assert_eq!(snapshot.metrics.event_overflows(), 1);
    assert_eq!(ends.events.drain().count(), capacity);
}
