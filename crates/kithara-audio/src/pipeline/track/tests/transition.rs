use kithara_abr::{AbrMode, AbrReason, AbrState, VariantIndex};
use kithara_decode::{
    DecodeResult, Decoder, DecoderChunkOutcome, DecoderFactory as DecoderBuilder,
    DecoderSeekOutcome, GaplessMode, PcmSpec,
};
use kithara_events::{DecoderChangeCause, DecoderEvent, DeferredBus, Event, EventBus};
use kithara_platform::{
    sync::{Arc, Mutex},
    time::Duration,
    tokio::task::yield_now,
};
use kithara_stream::{
    AudioCodec, ContainerFormat, MediaInfo, PendingReason, VariantPromotion, VariantReaderPlan,
    VariantTransition, VariantTransitionId,
};
use kithara_test_utils::kithara;

use super::rebuild::{
    Consts, RouteFixture, TestDecoder, media_info, produced_data, route_signal_source,
};
use crate::{
    pipeline::{
        decode::DecoderGeneration,
        rebuild::{DecoderBuildComplete, DecoderBuildPurpose, state::BuildId},
        seek::{ResumeState, SeekContext},
        track::{AtEof, CurrentFsm, Failed, Track, TrackFailure, TrackStep},
    },
    renderer::AudioWorkerSource,
};

fn incoming_plan() -> VariantReaderPlan {
    let abr = AbrState::new(AbrMode::Auto(Some(VariantIndex::new(0))));
    request_incoming_plan(&abr)
}

fn successive_incoming_plans() -> (VariantReaderPlan, VariantReaderPlan) {
    let abr = AbrState::new(AbrMode::Auto(Some(VariantIndex::new(0))));
    let first = request_incoming_plan(&abr);
    let second = request_incoming_plan(&abr);
    (first, second)
}

fn request_incoming_plan(abr: &AbrState) -> VariantReaderPlan {
    abr.request_target(VariantIndex::new(1), AbrReason::ManualOverride);
    let claim = abr
        .claim_pending_decision(VariantIndex::new(0))
        .expect("exact transition fixture requires a pending ABR claim");
    let transition = VariantTransition::new(
        VariantTransitionId::new(claim.ticket(), 0),
        VariantIndex::new(0),
        VariantIndex::new(1),
    );
    VariantReaderPlan::new(transition, media_info(1), Duration::ZERO)
}

async fn wait_for_incoming_priming(fixture: &mut RouteFixture, transition: VariantTransition) {
    for _ in 0..64 {
        yield_now().await;
        fixture.source.flush_deferred();
        if fixture.source.decode.incoming_is_priming(transition) {
            return;
        }
    }
    panic!("incoming decoder build did not complete");
}

#[kithara::test(tokio)]
async fn eof_transition_retires_staged_incoming_and_aborts_variant() {
    let mut fixture = route_signal_source(Consts::SAMPLE_RATE).await;
    let plan = incoming_plan();
    let transition = plan.transition();
    fixture.control.set_exact_plan(plan);
    fixture.control.set_exact_reader_ready();

    fixture.source.flush_deferred();
    wait_for_incoming_priming(&mut fixture, transition).await;

    assert!(fixture.source.decode.incoming_is_priming(transition));
    assert_eq!(fixture.control.aborted_transition(), None);
    assert!(fixture.drops.lock().is_empty());

    fixture.source.update_state(Track::<AtEof>::new(()).erase());
    assert!(matches!(fixture.source.state, CurrentFsm::AtEof(_)));
    fixture.source.flush_deferred();

    assert!(fixture.source.decode.incoming_transition().is_none());
    assert_eq!(fixture.control.aborted_transition(), Some(transition));
    assert_eq!(fixture.drops.lock().as_slice(), &[99]);
}

#[kithara::test(tokio)]
async fn failed_source_removal_retires_staged_incoming_and_aborts_variant() {
    let mut fixture = route_signal_source(Consts::SAMPLE_RATE).await;
    let plan = incoming_plan();
    let transition = plan.transition();
    fixture.control.set_exact_plan(plan);
    fixture.control.set_exact_reader_ready();

    fixture.source.flush_deferred();
    wait_for_incoming_priming(&mut fixture, transition).await;

    assert!(fixture.source.decode.incoming_is_priming(transition));
    assert_eq!(fixture.control.aborted_transition(), None);
    assert!(fixture.drops.lock().is_empty());

    fixture
        .source
        .update_state(Track::<Failed>::new(TrackFailure::SourceCancelled).erase());
    let control = fixture.control.clone();
    let drops = fixture.drops.clone();
    drop(fixture.source);

    assert_eq!(control.aborted_transition(), Some(transition));
    assert_eq!(drops.lock().as_slice(), &[99, 1]);
}

#[kithara::test(tokio)]
async fn exact_incoming_reader_pending_keeps_outgoing_pcm_running() {
    let mut fixture = route_signal_source(Consts::SAMPLE_RATE).await;
    fixture.control.set_exact_plan(incoming_plan());

    fixture.source.flush_deferred();

    let TrackStep::Produced(fetch) = fixture.source.step_track() else {
        panic!("outgoing must keep producing while the incoming reader is preparing");
    };
    let chunk = produced_data(fetch);
    assert!(!chunk.samples.is_empty());
    assert_eq!(chunk.meta.frame_offset, 0);
    assert_eq!(fixture.control.plan_calls(), 1);
    assert_eq!(fixture.control.prepare_calls(), 1);
    assert_eq!(fixture.control.take_calls(), 1);
    assert_eq!(
        fixture
            .source
            .decode
            .active()
            .media_info()
            .and_then(|info| info.variant_index),
        Some(0)
    );
    assert!(matches!(fixture.source.state, CurrentFsm::Decoding(_)));
}

#[kithara::test(tokio)]
async fn exact_incoming_build_pending_keeps_outgoing_pcm_running() {
    let mut fixture = route_signal_source(Consts::SAMPLE_RATE).await;
    let plan = incoming_plan();
    let transition = plan.transition();
    fixture.control.set_exact_plan(plan);
    fixture.control.set_exact_reader_ready();

    fixture.source.flush_deferred();

    assert!(fixture.source.decode.incoming_is_building(transition));
    let TrackStep::Produced(fetch) = fixture.source.step_track() else {
        panic!("outgoing must keep producing while the incoming decoder is building");
    };
    let chunk = produced_data(fetch);
    assert!(!chunk.samples.is_empty());
    assert_eq!(chunk.meta.frame_offset, 0);
    assert_eq!(fixture.control.plan_calls(), 1);
    assert_eq!(fixture.control.prepare_calls(), 1);
    assert_eq!(fixture.control.take_calls(), 1);
    assert_eq!(
        fixture
            .source
            .decode
            .active()
            .media_info()
            .and_then(|info| info.variant_index),
        Some(0)
    );
    assert!(matches!(fixture.source.state, CurrentFsm::Decoding(_)));
}

#[kithara::test(tokio)]
async fn incoming_completion_never_replaces_active_before_staged_pcm() {
    let mut fixture = route_signal_source(Consts::SAMPLE_RATE).await;
    let plan = incoming_plan();
    let transition = plan.transition();
    fixture.control.set_exact_plan(plan);
    fixture.control.set_exact_reader_ready();

    fixture.source.flush_deferred();
    wait_for_incoming_priming(&mut fixture, transition).await;

    assert_eq!(fixture.control.promote_calls(), 0);
    assert_eq!(
        fixture
            .source
            .decode
            .active()
            .media_info()
            .and_then(|info| info.variant_index),
        Some(0)
    );
    let TrackStep::Produced(fetch) = fixture.source.step_track() else {
        panic!("outgoing must remain authoritative while incoming PCM is staged");
    };
    let chunk = produced_data(fetch);
    assert_eq!(chunk.meta.frame_offset, 0);
    assert_eq!(fixture.control.promote_calls(), 0);
    assert_eq!(
        fixture
            .source
            .decode
            .active()
            .media_info()
            .and_then(|info| info.variant_index),
        Some(0)
    );
}

#[kithara::test(tokio)]
async fn exact_primed_generation_promotes_once_at_outgoing_frontier() {
    let mut fixture = route_signal_source(Consts::SAMPLE_RATE).await;
    let plan = incoming_plan();
    let transition = plan.transition();
    fixture.control.set_exact_plan(plan);
    fixture.control.set_exact_reader_ready();
    fixture.control.set_promotion(VariantPromotion::Promoted);

    fixture.source.flush_deferred();
    wait_for_incoming_priming(&mut fixture, transition).await;

    let TrackStep::Produced(outgoing) = fixture.source.step_track() else {
        panic!("outgoing must remain authoritative while incoming PCM is first staged");
    };
    assert_eq!(produced_data(outgoing).meta.frame_offset, 0);
    assert_eq!(fixture.control.promote_calls(), 0);

    fixture.source.flush_deferred();

    assert_eq!(fixture.control.promote_calls(), 1);
    assert_eq!(
        fixture
            .source
            .decode
            .active()
            .media_info()
            .and_then(|info| info.variant_index),
        Some(1)
    );
    let TrackStep::Produced(incoming) = fixture.source.step_track() else {
        panic!("promoted incoming must emit its already staged first chunk");
    };
    assert_eq!(
        produced_data(incoming).meta.frame_offset,
        u64::try_from(Consts::ROUTE_CHUNK_FRAMES).unwrap_or(u64::MAX)
    );
}

/// The incoming lands *on* the outgoing decode frontier, never ahead of it.
///
/// A promotion proof is only minted once the outgoing walks into the incoming's
/// staged span, and the outgoing stops decoding the moment its PCM ring is
/// full: a landing placed ahead of the frontier waits on motion nobody owes it,
/// and the switch never commits. The frontier advances one whole chunk at a
/// time here, so a landing that is not a whole number of chunks is a landing
/// that was pushed past it.
#[kithara::test(tokio)]
async fn incoming_lands_on_the_outgoing_frontier_not_ahead_of_it() {
    let mut fixture = route_signal_source(Consts::SAMPLE_RATE).await;
    let plan = incoming_plan();
    fixture.control.set_exact_plan(plan);
    fixture.control.set_exact_reader_ready();

    // A frontier has to exist before it can be landed against: until the
    // outgoing decodes, the source plans with no landing at all.
    let mut landing = None;
    for _ in 0..64 {
        yield_now().await;
        fixture.source.flush_deferred();
        landing = fixture.control.landing();
        if landing.is_some() {
            break;
        }
        if !matches!(fixture.source.step_track(), TrackStep::Produced(_)) {
            break;
        }
    }

    let landing = landing.expect("an exact transition plans against the outgoing frontier");
    let frames = (landing.as_secs_f64() * f64::from(Consts::SAMPLE_RATE)).round() as u64;
    let chunk = u64::try_from(Consts::ROUTE_CHUNK_FRAMES).unwrap_or(u64::MAX);
    assert_eq!(
        frames % chunk,
        0,
        "landing {landing:?} is {frames} frames — not a whole chunk, so it sits past \
         the frontier the promotion proof is stated against"
    );
}

#[kithara::test(tokio)]
async fn locked_promotion_keeps_primed_incoming_and_outgoing_authoritative() {
    let mut fixture = route_signal_source(Consts::SAMPLE_RATE).await;
    let plan = incoming_plan();
    let transition = plan.transition();
    fixture.control.set_exact_plan(plan);
    fixture.control.set_exact_reader_ready();
    fixture.control.set_promotion(VariantPromotion::Deferred);

    fixture.source.flush_deferred();
    wait_for_incoming_priming(&mut fixture, transition).await;

    let TrackStep::Produced(first_outgoing) = fixture.source.step_track() else {
        panic!("outgoing must remain audible while incoming PCM is first staged");
    };
    assert_eq!(produced_data(first_outgoing).meta.frame_offset, 0);

    fixture.source.flush_deferred();

    assert_eq!(fixture.control.promote_calls(), 1);
    assert!(fixture.source.decode.incoming_is_priming(transition));
    assert_eq!(
        fixture
            .source
            .decode
            .active()
            .media_info()
            .and_then(|info| info.variant_index),
        Some(0)
    );
    let TrackStep::Produced(next_outgoing) = fixture.source.step_track() else {
        panic!("publication lock must not interrupt outgoing PCM");
    };
    assert_eq!(
        produced_data(next_outgoing).meta.frame_offset,
        u64::try_from(Consts::ROUTE_CHUNK_FRAMES).unwrap_or(u64::MAX)
    );
}

#[kithara::test(tokio)]
async fn newer_ticket_supersedes_only_incoming_generation() {
    let mut fixture = route_signal_source(Consts::SAMPLE_RATE).await;
    let (first, second) = successive_incoming_plans();
    let first_transition = first.transition();
    let second_transition = second.transition();
    fixture.control.set_exact_plan(first);
    fixture.control.set_exact_reader_ready();

    fixture.source.flush_deferred();
    wait_for_incoming_priming(&mut fixture, first_transition).await;
    let TrackStep::Produced(outgoing) = fixture.source.step_track() else {
        panic!("outgoing must remain audible while the first incoming generation is staged");
    };
    assert_eq!(produced_data(outgoing).meta.frame_offset, 0);

    fixture.control.set_exact_plan(second);
    fixture.control.set_exact_reader_ready();
    fixture.source.flush_deferred();

    assert!(
        fixture
            .source
            .decode
            .incoming_is_building(second_transition)
    );
    assert_eq!(
        fixture
            .source
            .decode
            .active()
            .media_info()
            .and_then(|info| info.variant_index),
        Some(0)
    );
    assert_eq!(fixture.drops.lock().as_slice(), &[99]);
}

#[kithara::test(tokio)]
async fn stale_incoming_completion_retires_generation_in_shell() {
    let mut fixture = route_signal_source(Consts::SAMPLE_RATE).await;
    let (first, second) = successive_incoming_plans();
    let first_transition = first.transition();
    let second_transition = second.transition();
    let first_build = BuildId::fixture(41);

    assert!(
        fixture
            .source
            .decode
            .begin_incoming(first_transition)
            .is_none()
    );
    assert!(
        fixture
            .source
            .decode
            .mark_incoming_building(first_transition, first_build)
    );
    assert!(
        fixture
            .source
            .decode
            .begin_incoming(second_transition)
            .is_none()
    );

    let pushed = fixture
        .source
        .rebuild
        .completion()
        .push(DecoderBuildComplete {
            build: first_build,
            purpose: DecoderBuildPurpose::Incoming(first_transition),
            result: Ok(DecoderGeneration::new(
                Box::new(TestDecoder::new(77, fixture.drops.clone())),
                Some(media_info(1)),
                0,
                0,
                None,
                GaplessMode::Disabled,
            )),
        });
    assert!(pushed.is_ok());
    assert!(fixture.drops.lock().is_empty());

    fixture.source.flush_deferred();

    assert_eq!(fixture.drops.lock().as_slice(), &[77]);
    assert_eq!(
        fixture
            .source
            .decode
            .active()
            .media_info()
            .and_then(|info| info.variant_index),
        Some(0)
    );
}

#[kithara::test(tokio)]
async fn incoming_media_facts_choose_reader_profile() {
    let mut fixture = route_signal_source(Consts::SAMPLE_RATE).await;
    fixture.control.enable_byte_map();
    let template = incoming_plan();
    let mut incoming_media =
        MediaInfo::new(Some(AudioCodec::Mp3), Some(ContainerFormat::MpegAudio));
    incoming_media.variant_index = Some(1);
    let plan = VariantReaderPlan::new(
        template.transition(),
        incoming_media.clone(),
        template.landing_time(),
    );
    fixture.control.set_exact_plan(plan);

    fixture.source.flush_deferred();

    let byte_map = fixture.source.shared_stream.byte_map();
    let expected = DecoderBuilder::reader_profile(&incoming_media, byte_map.as_deref());
    let active = DecoderBuilder::reader_profile(&media_info(0), byte_map.as_deref());
    assert_ne!(expected, active);
    assert_eq!(fixture.control.prepared_profile(), Some(expected));
}

#[kithara::test(tokio)]
async fn exact_promotion_emits_variant_switch_decoder_event() {
    let mut fixture = route_signal_source(Consts::SAMPLE_RATE).await;
    let bus = EventBus::new(16);
    let mut events = bus.subscribe();
    fixture.source = fixture
        .source
        .with_emit(Arc::new(DeferredBus::new(bus, 16)));
    let plan = incoming_plan();
    let transition = plan.transition();
    fixture.control.set_exact_plan(plan);
    fixture.control.set_exact_reader_ready();
    fixture.control.set_promotion(VariantPromotion::Promoted);

    fixture.source.flush_deferred();
    wait_for_incoming_priming(&mut fixture, transition).await;
    assert!(matches!(
        fixture.source.step_track(),
        TrackStep::Produced(_)
    ));
    fixture.source.flush_deferred();

    let mut changed = false;
    while let Ok(envelope) = events.try_recv() {
        changed |= matches!(
            envelope.event,
            Event::Decoder(DecoderEvent::DecoderChanged {
                cause: DecoderChangeCause::VariantSwitch,
                variant: Some(1),
                ..
            })
        );
    }
    assert!(changed);
}

struct RelandPendingDecoder {
    inner: TestDecoder,
    seek_target: Arc<Mutex<Option<Duration>>>,
}

impl Decoder for RelandPendingDecoder {
    fn duration(&self) -> Option<Duration> {
        self.inner.duration()
    }

    fn next_chunk(&mut self) -> DecodeResult<DecoderChunkOutcome> {
        Ok(DecoderChunkOutcome::Pending(PendingReason::Retry))
    }

    fn seek(&mut self, pos: Duration) -> DecodeResult<DecoderSeekOutcome> {
        *self.seek_target.lock() = Some(pos);
        self.inner.seek(pos)
    }

    fn spec(&self) -> PcmSpec {
        self.inner.spec()
    }

    fn update_byte_len(&self, len: u64) {
        self.inner.update_byte_len(len);
    }
}

#[kithara::test(tokio)]
async fn unstaged_priming_generation_relands_to_current_outgoing_frontier() {
    let mut fixture = route_signal_source(Consts::SAMPLE_RATE).await;
    let plan = incoming_plan();
    let transition = plan.transition();
    fixture.control.set_exact_plan(plan);

    let TrackStep::Produced(outgoing) = fixture.source.step_track() else {
        panic!("outgoing must establish an exact decode frontier");
    };
    let expected_landing = produced_data(outgoing).meta.end_timestamp;
    let seek_target = Arc::new(Mutex::new(None));
    let build = BuildId::fixture(42);
    assert!(fixture.source.decode.begin_incoming(transition).is_none());
    assert!(
        fixture
            .source
            .decode
            .mark_incoming_building(transition, build)
    );
    let generation = DecoderGeneration::new(
        Box::new(RelandPendingDecoder {
            inner: TestDecoder::new(77, fixture.drops.clone()),
            seek_target: seek_target.clone(),
        }),
        Some(media_info(1)),
        0,
        transition.id().seek_epoch(),
        Some(ResumeState {
            trim_head: true,
            seek: SeekContext {
                target: Duration::ZERO,
                epoch: transition.id().seek_epoch(),
            },
            ..Default::default()
        }),
        GaplessMode::Disabled,
    );
    assert!(
        fixture
            .source
            .decode
            .install_incoming(transition, build, generation)
            .is_none()
    );
    assert!(fixture.source.decode.incoming_is_priming(transition));

    fixture.source.flush_deferred();

    assert!(fixture.source.decode.incoming_is_relanding(transition));
    wait_for_incoming_priming(&mut fixture, transition).await;

    assert!(fixture.source.decode.incoming_is_priming(transition));
    assert_eq!(*seek_target.lock(), Some(expected_landing));
}
